use std::{
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{Histogram, Log2Histogram, MantissaHistogram, MetricStorage, consumer, histogram};

pub type FineHistogram = MantissaHistogram<976, 4>;

#[derive(Clone, Copy, Debug)]
pub enum HistogramBins<'a> {
    U32(&'a [u32]),
    U64(&'a [u64]),
}

impl HistogramBins<'_> {
    pub(crate) fn sample_count(self) -> u64 {
        match self {
            Self::U32(bins) => bins.iter().copied().map(u64::from).sum(),
            Self::U64(bins) => bins.iter().copied().sum(),
        }
    }

    pub(crate) fn get(self, index: usize) -> u64 {
        match self {
            Self::U32(bins) => u64::from(bins[index]),
            Self::U64(bins) => bins[index],
        }
    }

    pub(crate) fn len(self) -> usize {
        match self {
            Self::U32(bins) => bins.len(),
            Self::U64(bins) => bins.len(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SummaryValue {
    pub count: u64,
    pub sum: u64,
    pub min: u64,
    pub max: u64,
}

#[derive(Clone, Copy, Debug)]
pub enum ValueRef<'a> {
    Histogram(histogram::Ref<'a>),
    Counter(u64),
    Gauge(u64),
    Summary(SummaryValue),
}

impl ValueRef<'_> {
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Histogram(_) => "histogram",
            Self::Counter(_) => "counter",
            Self::Gauge(_) => "gauge",
            Self::Summary(_) => "summary",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Ref<'a> {
    pub scope: &'static str,
    pub name: &'static str,
    pub value: ValueRef<'a>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub name: &'static str,
    pub scope: &'static str,
    pub storage: MetricStorage,
    pub histogram: histogram::Snapshot,
}

impl Snapshot {
    pub fn as_ref(&self) -> Ref<'_> {
        Ref {
            name: self.name,
            scope: self.scope,
            value: match self.histogram.layout {
                histogram::Layout::Count => ValueRef::Counter(self.histogram.sample_count),
                _ => ValueRef::Histogram(self.histogram.as_ref()),
            },
        }
    }
}

pub struct Descriptor {
    pub name: &'static str,
    pub scope: &'static str,
    pub layout: histogram::Layout,
    pub storage: MetricStorage,
    pub(crate) warm: fn(),
    pub(crate) access: fn() -> Option<histogram::Snapshot>,
    fresh_accesses: &'static AtomicU64,
    consumption_calls: &'static AtomicU64,
    consumed_samples: &'static AtomicU64,
}

impl Descriptor {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        name: &'static str,
        scope: &'static str,
        layout: histogram::Layout,
        storage: MetricStorage,
        warm: fn(),
        access: fn() -> Option<histogram::Snapshot>,
        fresh_accesses: &'static AtomicU64,
        consumption_calls: &'static AtomicU64,
        consumed_samples: &'static AtomicU64,
    ) -> Self {
        Self {
            name,
            scope,
            layout,
            storage,
            warm,
            access,
            fresh_accesses,
            consumption_calls,
            consumed_samples,
        }
    }

    pub fn consumption_status(&self) -> consumer::Status {
        consumer::Status {
            name: self.name,
            scope: self.scope,
            storage: self.storage,
            fresh_accesses: self.fresh_accesses.load(Ordering::Relaxed),
            consumption_calls: self.consumption_calls.load(Ordering::Relaxed),
            consumed_samples: self.consumed_samples.load(Ordering::Relaxed),
        }
    }
}

pub trait Metric {
    fn value(&self) -> Option<ValueRef<'_>>;
}

pub trait Set {
    const SCOPE: &'static str;

    fn visit<'a>(&'a self, visitor: &mut dyn FnMut(Ref<'a>));
}

pub trait Key: Copy {
    const SCOPE: &'static str;
    const COUNT: usize;

    fn index(self) -> usize;
    fn name(index: usize) -> &'static str;
}

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistogramSet<K, H, const N: usize> {
    histograms: [H; N],
    key: PhantomData<fn(K)>,
}

impl<K: Key, H, const N: usize> HistogramSet<K, H, N> {
    pub const fn new_const(histograms: [H; N]) -> Self {
        assert!(N == K::COUNT, "histogram count must match its metric key");
        Self {
            histograms,
            key: PhantomData,
        }
    }
}

impl<K: Key, H: Default, const N: usize> Default for HistogramSet<K, H, N> {
    fn default() -> Self {
        Self::new_const(std::array::from_fn(|_| H::default()))
    }
}

impl<K: Key, H: Histogram, const N: usize> HistogramSet<K, H, N> {
    #[inline(always)]
    pub fn record(&mut self, key: K, delta_cycles: u64) {
        self.histograms[key.index()].record(delta_cycles);
    }

    pub fn histogram(&self, key: K) -> &H {
        &self.histograms[key.index()]
    }
}

impl<K: Key, H: Metric, const N: usize> Set for HistogramSet<K, H, N> {
    const SCOPE: &'static str = K::SCOPE;

    fn visit<'a>(&'a self, visitor: &mut dyn FnMut(Ref<'a>)) {
        for (index, histogram) in self.histograms.iter().enumerate() {
            if let Some(value) = histogram.value() {
                visitor(Ref {
                    scope: K::SCOPE,
                    name: K::name(index),
                    value,
                });
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counter {
    value: u64,
}

impl Counter {
    #[inline(always)]
    pub fn increment(&mut self) {
        self.add(1);
    }

    #[inline(always)]
    pub fn add(&mut self, value: u64) {
        self.value = self.value.wrapping_add(value);
    }

    pub const fn get(&self) -> u64 {
        self.value
    }
}

impl Metric for Counter {
    fn value(&self) -> Option<ValueRef<'_>> {
        Some(ValueRef::Counter(self.value))
    }
}

#[derive(Debug, Default)]
pub struct AtomicCounter {
    value: AtomicU64,
}

impl AtomicCounter {
    #[inline(always)]
    pub fn increment(&self) {
        self.add(1);
    }

    #[inline(always)]
    pub fn add(&self, value: u64) {
        self.value.fetch_add(value, Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }
}

impl Metric for AtomicCounter {
    fn value(&self) -> Option<ValueRef<'_>> {
        Some(ValueRef::Counter(self.get()))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gauge {
    value: u64,
}

impl Gauge {
    #[inline(always)]
    pub fn set(&mut self, value: u64) {
        self.value = value;
    }

    pub const fn get(&self) -> u64 {
        self.value
    }
}

impl Metric for Gauge {
    fn value(&self) -> Option<ValueRef<'_>> {
        Some(ValueRef::Gauge(self.value))
    }
}

#[derive(Debug, Default)]
pub struct AtomicGauge {
    value: AtomicU64,
}

impl AtomicGauge {
    #[inline(always)]
    pub fn set(&self, value: u64) {
        self.value.store(value, Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }
}

impl Metric for AtomicGauge {
    fn value(&self) -> Option<ValueRef<'_>> {
        Some(ValueRef::Gauge(self.get()))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    count: u64,
    sum: u64,
    min: u64,
    max: u64,
}

impl Summary {
    #[inline(always)]
    pub fn record(&mut self, value: u64) {
        if self.count == 0 {
            self.min = value;
        } else {
            self.min = self.min.min(value);
        }
        self.count = self.count.wrapping_add(1);
        self.sum = self.sum.wrapping_add(value);
        self.max = self.max.max(value);
    }

    pub const fn get(&self) -> SummaryValue {
        SummaryValue {
            count: self.count,
            sum: self.sum,
            min: self.min,
            max: self.max,
        }
    }
}

impl Metric for Summary {
    fn value(&self) -> Option<ValueRef<'_>> {
        (self.count != 0).then(|| ValueRef::Summary(self.get()))
    }
}

#[derive(Debug)]
pub struct AtomicSummary {
    count: AtomicU64,
    sum: AtomicU64,
    min: AtomicU64,
    max: AtomicU64,
}

impl AtomicSummary {
    pub const fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            sum: AtomicU64::new(0),
            min: AtomicU64::new(u64::MAX),
            max: AtomicU64::new(0),
        }
    }

    #[inline(always)]
    pub fn record(&self, value: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum.fetch_add(value, Ordering::Relaxed);
        self.min.fetch_min(value, Ordering::Relaxed);
        self.max.fetch_max(value, Ordering::Relaxed);
    }

    pub fn get(&self) -> SummaryValue {
        let count = self.count.load(Ordering::Relaxed);
        SummaryValue {
            count,
            sum: self.sum.load(Ordering::Relaxed),
            min: if count == 0 {
                0
            } else {
                self.min.load(Ordering::Relaxed)
            },
            max: self.max.load(Ordering::Relaxed),
        }
    }
}

impl Default for AtomicSummary {
    fn default() -> Self {
        Self::new()
    }
}

impl Metric for AtomicSummary {
    fn value(&self) -> Option<ValueRef<'_>> {
        let value = self.get();
        (value.count != 0).then_some(ValueRef::Summary(value))
    }
}

impl Metric for Log2Histogram {
    fn value(&self) -> Option<ValueRef<'_>> {
        let bins = HistogramBins::U64(&self.bins);
        let sample_count = bins.sample_count();
        (sample_count != 0).then_some(ValueRef::Histogram(histogram::Ref {
            layout: histogram::Layout::Log2,
            bins,
            sample_count,
        }))
    }
}

impl<const BINS: usize, const SUB_BUCKET_BITS: usize> Metric
    for MantissaHistogram<BINS, SUB_BUCKET_BITS>
{
    fn value(&self) -> Option<ValueRef<'_>> {
        let bins = HistogramBins::U32(&self.bins);
        let sample_count = bins.sample_count();
        (sample_count != 0).then_some(ValueRef::Histogram(histogram::Ref {
            layout: <Self as Histogram>::LAYOUT,
            bins,
            sample_count,
        }))
    }
}
