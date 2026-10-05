//! Compile-time registered telemetry for latency-sensitive code.
//!
//! Instrumentation sites own const-initialized shared atomic histograms by
//! default. The recording path performs no allocation, locking, formatting,
//! registration, or unit conversion. Link-time descriptors provide cold
//! discovery, warm-up, access, and consumption accounting without a
//! handwritten metric bank.
//!
//! Instrumentation is present only when the instrumented package enables its
//! `brap` Cargo feature. An optional `set = "name"` additionally requires
//! that package's generated `brap-name` feature. Set features must be
//! declared explicitly in the instrumented package because procedural macros
//! cannot edit its manifest. A conventional feature surface is:
//!
//! ```toml
//! [features]
//! default = ["brap-all"]
//! brap = ["brap/runtime"]
//! brap-all = ["brap-pipeline"]
//! brap-pipeline = ["brap"]
//!
//! [dependencies]
//! brap = { version = "...", default-features = false }
//! ```
//!
//! A single startup consumer automatically discovers, drains, and logs shared
//! sites. Its interval, scope, and callback are configurable:
//!
//! ```
//! use std::time::Duration;
//!
//! const SCOPE: &str = "brap-example";
//!
//! #[brap::measure_fine(name = "example-total", scope = SCOPE)]
//! fn example() {
//!     example_stage_1();
//!     brap::checkpoint!("example-stage-1");
//!     example_stage_2();
//!     brap::checkpoint!("example-stage-2");
//! }
//!
//! # fn example_stage_1() {}
//! # fn example_stage_2() {}
//!
//! #[derive(Default, brap::MetricSet)]
//! #[brap(scope = "brap-example")]
//! struct ExampleMetrics {
//!     example_counter: brap::Counter,
//!     example_histogram: brap::FineHistogram,
//! }
//! let mut metrics = ExampleMetrics::default();
//! metrics.example_counter.increment();
//! metrics.example_histogram.record(64);
//!
//! let _consumer = brap::consumer::build()
//!     .scope(SCOPE)
//!     .every(Duration::from_secs(10))
//!     .thread_name("brap-example")
//!     .consume(|_| {})
//!     .spawn()
//!     .expect("start telemetry consumer");
//! example();
//! brap::record_log2!(SCOPE, "example-span", 42);
//! ```
//!
//! [`checkpoint!`] is available inside a measured function for consecutive
//! stages. Each checkpoint inherits the enclosing measurement's scope,
//! compile set, histogram layout and precision, and storage. It records the
//! cycles since function entry or the previously executed checkpoint, while
//! the function's own metric continues to record the complete call.
//! [`record!`] declares the same kind of site for a cycle delta that already
//! exists at the instrumentation point. Recording never allocates, locks,
//! logs, formats, or discovers metrics. Snapshot access counts as consumption
//! regardless of what the caller subsequently does with the snapshot.
//! `storage = "tls"` selects the cheaper TLS backend for exceptionally hot
//! single-owner loops; those sites require a [`TlsOwnerGuard`] on the producer
//! thread. A nonzero
//! [`consumer::Status::fresh_accesses`] value means a TLS site was first
//! reached on a thread that had not warmed it. [`stamp!`] conditionally reads
//! the cycle clock under the same feature predicate, so explicitly delimited
//! spans retain no clock read when their telemetry set is disabled.

extern crate self as brap;

pub mod clock;
pub mod consumer;
pub mod histogram;
pub mod metric;
mod output;

use std::{
    cell::{Cell, UnsafeCell},
    mem,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

pub use metric::{
    AtomicCounter, AtomicGauge, AtomicSummary, Counter, FineHistogram, Gauge, HistogramBins,
    HistogramSet, Metric, Summary, SummaryValue, ValueRef,
};
pub use output::{Field, FieldValue, log_metric, log_set, write_metric, write_set};
pub use brap_macros::{
    MetricKey, MetricSet, checkpoint, measure, measure_fine, record, stamp,
};

#[doc(hidden)]
#[cfg(feature = "runtime")]
pub mod __private {
    pub use linkme::*;
}

/// Declares and records a shared atomic 64-bin logarithmic cycle histogram at
/// this callsite.
#[macro_export]
macro_rules! record_log2 {
    (
        $scope:expr,
        $name:literal,
        $cycles:expr
        $(, set = $set:literal)?
        $(, storage = $storage:literal)?
        $(,)?
    ) => {
        $crate::record!(
            name = $name,
            scope = $scope,
            $(set = $set,)?
            histogram = "log2",
            $(storage = $storage,)?
            cycles = $cycles
        )
    };
    (
        $name:literal,
        $cycles:expr
        $(, set = $set:literal)?
        $(, storage = $storage:literal)?
        $(,)?
    ) => {
        $crate::record!(
            name = $name,
            $(set = $set,)?
            histogram = "log2",
            $(storage = $storage,)?
            cycles = $cycles
        )
    };
}

/// Declares and records a shared atomic mantissa cycle histogram at this
/// callsite.
/// Precision defaults to four sub-bucket bits.
#[macro_export]
macro_rules! record_fine {
    (
        $scope:expr,
        $name:literal,
        $cycles:expr
        $(, precision = $precision:literal)?
        $(, set = $set:literal)?
        $(, storage = $storage:literal)?
        $(,)?
    ) => {
        $crate::record!(
            name = $name,
            scope = $scope,
            $(set = $set,)?
            histogram = "fine",
            $(precision = $precision,)?
            $(storage = $storage,)?
            cycles = $cycles
        )
    };
    (
        $name:literal,
        $cycles:expr
        $(, precision = $precision:literal)?
        $(, set = $set:literal)?
        $(, storage = $storage:literal)?
        $(,)?
    ) => {
        $crate::record!(
            name = $name,
            $(set = $set,)?
            histogram = "fine",
            $(precision = $precision,)?
            $(storage = $storage,)?
            cycles = $cycles
        )
    };
}

/// Declares a counter at this callsite and adds `value` to it, or one without
/// a value. Counts are discovered and drained like histogram sites and log as
/// `kind=counter`; the snapshot's `sample_count` is the count. When the site
/// is disabled, `value` is not evaluated.
#[macro_export]
macro_rules! count {
    (
        $scope:expr,
        $name:literal
        $(, set = $set:literal)?
        $(, storage = $storage:literal)?
        $(,)?
    ) => {
        $crate::count!($scope, $name, 1 $(, set = $set)? $(, storage = $storage)?)
    };
    (
        $scope:expr,
        $name:literal,
        $value:expr
        $(, set = $set:literal)?
        $(, storage = $storage:literal)?
        $(,)?
    ) => {
        $crate::record!(
            name = $name,
            scope = $scope,
            $(set = $set,)?
            histogram = "count",
            $(storage = $storage,)?
            cycles = $value
        )
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetricStorage {
    Atomic,
    Tls,
}

pub trait Histogram: Default + 'static {
    const LAYOUT: histogram::Layout;

    fn record(&mut self, delta_cycles: u64);
    fn sample_count(&self) -> u64;
    fn touch_pages(&mut self);
    fn take_bins(&mut self) -> Vec<u64>;
}

#[doc(hidden)]
pub trait AtomicHistogram: 'static {
    const LAYOUT: histogram::Layout;

    fn record(&self, delta_cycles: u64);
    fn touch_pages(&self);
    fn take_bins(&self) -> Option<(u64, Vec<u64>)>;
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Log2Histogram {
    bins: [u64; 64],
}

impl Log2Histogram {
    pub const fn new_const() -> Self {
        Self { bins: [0; 64] }
    }

    #[inline(always)]
    pub fn record(&mut self, delta_cycles: u64) {
        let index = 63 - (delta_cycles | 1).leading_zeros() as usize;
        self.bins[index] += 1;
    }

    pub fn sample_count(&self) -> u64 {
        self.bins.iter().copied().sum()
    }
}

impl Default for Log2Histogram {
    fn default() -> Self {
        Self::new_const()
    }
}

impl Histogram for Log2Histogram {
    const LAYOUT: histogram::Layout = histogram::Layout::Log2;

    #[inline(always)]
    fn record(&mut self, delta_cycles: u64) {
        Self::record(self, delta_cycles);
    }

    fn sample_count(&self) -> u64 {
        Self::sample_count(self)
    }

    fn touch_pages(&mut self) {
        warm_counter(&mut self.bins[0]);
        warm_counter(&mut self.bins[63]);
    }

    fn take_bins(&mut self) -> Vec<u64> {
        mem::replace(&mut self.bins, [0; 64]).into()
    }
}

#[repr(C)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MantissaHistogram<const BINS: usize, const SUB_BUCKET_BITS: usize> {
    bins: [u32; BINS],
}

impl<const BINS: usize, const SUB_BUCKET_BITS: usize> MantissaHistogram<BINS, SUB_BUCKET_BITS> {
    pub const fn new_const() -> Self {
        Self { bins: [0; BINS] }
    }

    const fn expected_bins() -> usize {
        let sub_buckets = 1usize << SUB_BUCKET_BITS;
        sub_buckets + (64 - SUB_BUCKET_BITS) * sub_buckets
    }

    #[inline(always)]
    pub fn record(&mut self, delta_cycles: u64) {
        debug_assert_eq!(BINS, Self::expected_bins());
        let sub_buckets = 1usize << SUB_BUCKET_BITS;
        let exponent = 63 - (delta_cycles | 1).leading_zeros() as usize;
        let index = if exponent < SUB_BUCKET_BITS {
            delta_cycles as usize
        } else {
            let shift = exponent - SUB_BUCKET_BITS;
            let sub_bucket = ((delta_cycles >> shift) as usize) & (sub_buckets - 1);
            sub_buckets + (exponent - SUB_BUCKET_BITS) * sub_buckets + sub_bucket
        };
        // BUG: `u32` bins overflow after ~4.3e9 samples in one bin between
        // drains (~90 s of a 20 ns hot loop); release builds wrap silently.
        self.bins[index] += 1;
    }

    pub fn sample_count(&self) -> u64 {
        self.bins.iter().copied().map(u64::from).sum()
    }
}

impl<const BINS: usize, const SUB_BUCKET_BITS: usize> Default
    for MantissaHistogram<BINS, SUB_BUCKET_BITS>
{
    fn default() -> Self {
        Self::new_const()
    }
}

impl<const BINS: usize, const SUB_BUCKET_BITS: usize> Histogram
    for MantissaHistogram<BINS, SUB_BUCKET_BITS>
{
    const LAYOUT: histogram::Layout = histogram::Layout::Mantissa {
        sub_bucket_bits: SUB_BUCKET_BITS as u8,
    };

    #[inline(always)]
    fn record(&mut self, delta_cycles: u64) {
        Self::record(self, delta_cycles);
    }

    fn sample_count(&self) -> u64 {
        Self::sample_count(self)
    }

    fn touch_pages(&mut self) {
        const U32S_PER_PAGE: usize = 4096 / mem::size_of::<u32>();
        for value in self.bins.iter_mut().step_by(U32S_PER_PAGE) {
            warm_counter(value);
        }
        if let Some(last) = self.bins.last_mut() {
            warm_counter(last);
        }
    }

    fn take_bins(&mut self) -> Vec<u64> {
        let bins = mem::replace(&mut self.bins, [0; BINS]);
        bins.into_iter().map(u64::from).collect()
    }
}

/// Thread-local storage for a `count!` site: the running total.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CountHistogram {
    total: u64,
}

impl CountHistogram {
    pub const fn new_const() -> Self {
        Self { total: 0 }
    }
}

impl Histogram for CountHistogram {
    const LAYOUT: histogram::Layout = histogram::Layout::Count;

    #[inline(always)]
    fn record(&mut self, value: u64) {
        self.total = self.total.wrapping_add(value);
    }

    fn sample_count(&self) -> u64 {
        self.total
    }

    fn touch_pages(&mut self) {
        warm_counter(&mut self.total);
    }

    fn take_bins(&mut self) -> Vec<u64> {
        vec![mem::take(&mut self.total)]
    }
}

/// Shared storage for a `count!` site: one sharded bin holding the total.
#[doc(hidden)]
#[derive(Debug)]
pub struct AtomicCountHistogram {
    bins: ShardedBins<1>,
}

impl AtomicCountHistogram {
    pub const fn new_const() -> Self {
        Self {
            bins: ShardedBins::new(),
        }
    }
}

impl AtomicHistogram for AtomicCountHistogram {
    const LAYOUT: histogram::Layout = histogram::Layout::Count;

    #[inline(always)]
    fn record(&self, value: u64) {
        self.bins.add(0, value);
    }

    fn touch_pages(&self) {
        self.bins.touch_pages();
    }

    fn take_bins(&self) -> Option<(u64, Vec<u64>)> {
        self.bins.take()
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct AtomicLog2Histogram {
    bins: ShardedBins<64>,
}

impl AtomicLog2Histogram {
    pub const fn new_const() -> Self {
        Self {
            bins: ShardedBins::new(),
        }
    }
}

impl Default for AtomicLog2Histogram {
    fn default() -> Self {
        Self::new_const()
    }
}

impl AtomicHistogram for AtomicLog2Histogram {
    const LAYOUT: histogram::Layout = histogram::Layout::Log2;

    #[inline(always)]
    fn record(&self, delta_cycles: u64) {
        let index = 63 - (delta_cycles | 1).leading_zeros() as usize;
        self.bins.add(index, 1);
    }

    fn touch_pages(&self) {
        self.bins.touch_pages();
    }

    fn take_bins(&self) -> Option<(u64, Vec<u64>)> {
        self.bins.take()
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct AtomicMantissaHistogram<const BINS: usize, const SUB_BUCKET_BITS: usize> {
    bins: ShardedBins<BINS>,
}

impl<const BINS: usize, const SUB_BUCKET_BITS: usize>
    AtomicMantissaHistogram<BINS, SUB_BUCKET_BITS>
{
    pub const fn new_const() -> Self {
        Self {
            bins: ShardedBins::new(),
        }
    }

    const fn expected_bins() -> usize {
        let sub_buckets = 1usize << SUB_BUCKET_BITS;
        sub_buckets + (64 - SUB_BUCKET_BITS) * sub_buckets
    }
}

impl<const BINS: usize, const SUB_BUCKET_BITS: usize> Default
    for AtomicMantissaHistogram<BINS, SUB_BUCKET_BITS>
{
    fn default() -> Self {
        Self::new_const()
    }
}

impl<const BINS: usize, const SUB_BUCKET_BITS: usize> AtomicHistogram
    for AtomicMantissaHistogram<BINS, SUB_BUCKET_BITS>
{
    const LAYOUT: histogram::Layout = histogram::Layout::Mantissa {
        sub_bucket_bits: SUB_BUCKET_BITS as u8,
    };

    #[inline(always)]
    fn record(&self, delta_cycles: u64) {
        debug_assert_eq!(BINS, Self::expected_bins());
        let sub_buckets = 1usize << SUB_BUCKET_BITS;
        let exponent = 63 - (delta_cycles | 1).leading_zeros() as usize;
        let index = if exponent < SUB_BUCKET_BITS {
            delta_cycles as usize
        } else {
            let shift = exponent - SUB_BUCKET_BITS;
            let sub_bucket = ((delta_cycles >> shift) as usize) & (sub_buckets - 1);
            sub_buckets + (exponent - SUB_BUCKET_BITS) * sub_buckets + sub_bucket
        };
        self.bins.add(index, 1);
    }

    fn touch_pages(&self) {
        self.bins.touch_pages();
    }

    fn take_bins(&self) -> Option<(u64, Vec<u64>)> {
        self.bins.take()
    }
}

pub struct AtomicSite<H> {
    histogram: H,
}

impl<H> AtomicSite<H> {
    pub const fn new(histogram: H) -> Self {
        Self { histogram }
    }
}

impl<H: AtomicHistogram> AtomicSite<H> {
    pub fn warm(&self) {
        self.histogram.touch_pages();
    }

    #[inline(always)]
    pub fn record(&self, delta_cycles: u64) {
        self.histogram.record(delta_cycles);
    }

    pub fn access(
        &self,
        consumption_calls: &AtomicU64,
        consumed_samples: &AtomicU64,
    ) -> Option<histogram::Snapshot> {
        let (sample_count, bins) = self.histogram.take_bins()?;
        consumption_calls.fetch_add(1, Ordering::Relaxed);
        consumed_samples.fetch_add(sample_count, Ordering::Relaxed);
        Some(histogram::Snapshot {
            layout: H::LAYOUT,
            sample_count,
            bins,
        })
    }
}

pub struct TlsSite<H> {
    histogram: UnsafeCell<H>,
    warmed: Cell<bool>,
}

impl<H> TlsSite<H> {
    pub const fn new(histogram: H) -> Self {
        Self {
            histogram: UnsafeCell::new(histogram),
            warmed: Cell::new(false),
        }
    }
}

impl<H: Histogram> TlsSite<H> {
    pub fn warm(&self) {
        // SAFETY: a TlsSite is stored in TLS and is accessible only by its
        // owner thread. Warm-up runs before the owner enters its hot loop.
        unsafe { &mut *self.histogram.get() }.touch_pages();
        self.warmed.set(true);
    }

    #[inline(always)]
    pub fn record(&self, delta_cycles: u64, fresh_accesses: &AtomicU64) {
        if !self.warmed.get() {
            self.warmed.set(true);
            fresh_accesses.fetch_add(1, Ordering::Relaxed);
        }
        // SAFETY: the site is thread-local, and access/snapshot operations are
        // required to run cooperatively on the same owner thread.
        unsafe { &mut *self.histogram.get() }.record(delta_cycles);
    }

    pub fn access(
        &self,
        consumption_calls: &AtomicU64,
        consumed_samples: &AtomicU64,
    ) -> Option<histogram::Snapshot> {
        // SAFETY: access is cooperative and runs on the TLS owner thread.
        let histogram = unsafe { &mut *self.histogram.get() };
        let sample_count = histogram.sample_count();
        if sample_count == 0 {
            return None;
        }
        let bins = histogram.take_bins();
        consumption_calls.fetch_add(1, Ordering::Relaxed);
        consumed_samples.fetch_add(sample_count, Ordering::Relaxed);
        Some(histogram::Snapshot {
            layout: H::LAYOUT,
            sample_count,
            bins,
        })
    }
}

#[cfg(feature = "runtime")]
#[linkme::distributed_slice]
pub static METRICS: [metric::Descriptor];

#[cfg(not(feature = "runtime"))]
pub static METRICS: [metric::Descriptor; 0] = [];

thread_local! {
    static CURRENT_OWNER: Cell<Option<&'static str>> = const { Cell::new(None) };
}

#[must_use = "the owner guard must remain alive while its thread records telemetry"]
pub struct TlsOwnerGuard {
    scope: &'static str,
}

impl TlsOwnerGuard {
    pub fn enter(scope: &'static str) -> Self {
        CURRENT_OWNER.with(|owner| {
            assert!(
                owner.get().is_none(),
                "telemetry owner is already registered"
            );
            owner.set(Some(scope));
        });
        Self { scope }
    }

    pub fn warm(self) -> Self {
        for descriptor in METRICS
            .iter()
            .filter(|metric| metric.scope == self.scope && metric.storage == MetricStorage::Tls)
        {
            (descriptor.warm)();
        }
        self
    }

    pub fn scope(&self) -> &'static str {
        self.scope
    }
}

impl Drop for TlsOwnerGuard {
    fn drop(&mut self) {
        CURRENT_OWNER.with(|owner| {
            debug_assert_eq!(owner.get(), Some(self.scope));
            owner.set(None);
        });
    }
}

/// Accesses and drains all non-empty sites in `scope`. Atomic sites are shared
/// process-wide; thread-local sites drain only the calling thread.
/// Calling this function is the consumption boundary.
pub fn access_scope(scope: &'static str) -> Vec<metric::Snapshot> {
    let mut snapshots = Vec::new();
    visit_scope(scope, |snapshot| snapshots.push(snapshot));
    snapshots
}

/// Visits and drains all non-empty sites in `scope`. Invoking the visitor
/// counts as consumption regardless of its behavior.
pub fn visit_scope(scope: &'static str, mut visitor: impl FnMut(metric::Snapshot)) {
    for descriptor in METRICS.iter().filter(|metric| metric.scope == scope) {
        if let Some(histogram) = (descriptor.access)() {
            visitor(metric::Snapshot {
                name: descriptor.name,
                scope: descriptor.scope,
                storage: descriptor.storage,
                histogram,
            });
        }
    }
}

pub fn log_snapshot(snapshot: metric::Snapshot) {
    if let Err(error) = log_metric(snapshot.as_ref(), &[]) {
        eprintln!("failed to write telemetry snapshot: {error}");
    }
}

fn atomic_descriptors(
    scope: Option<&'static str>,
) -> impl Iterator<Item = &'static metric::Descriptor> {
    METRICS.iter().filter(move |metric| {
        metric.storage == MetricStorage::Atomic && scope.is_none_or(|scope| metric.scope == scope)
    })
}

fn warm_atomic(scope: Option<&'static str>) {
    for descriptor in atomic_descriptors(scope) {
        (descriptor.warm)();
    }
}

fn visit_atomic(scope: Option<&'static str>, visitor: &mut dyn FnMut(metric::Snapshot)) {
    for descriptor in atomic_descriptors(scope) {
        if let Some(histogram) = (descriptor.access)() {
            visitor(metric::Snapshot {
                name: descriptor.name,
                scope: descriptor.scope,
                storage: descriptor.storage,
                histogram,
            });
        }
    }
}

/// Returns process-wide evidence that registered sites were accessed. For
/// thread-local sites it also reports first use before owner warm-up. Access is
/// the consumption boundary; no downstream acknowledgement is implied.
pub fn consumption_statuses() -> Vec<consumer::Status> {
    METRICS
        .iter()
        .map(metric::Descriptor::consumption_status)
        .collect()
}

#[inline(always)]
pub fn now_cycles() -> u64 {
    clock::now_cycles()
}

/// Copies of each shared histogram's bins. Threads take shards round-robin,
/// so up to this many recording threads never contend on a cache line. Set
/// at compile time with the `BRAP_SHARDS` environment variable; `1` disables
/// sharding.
const SHARDS: usize = match option_env!("BRAP_SHARDS") {
    Some(value) => parse_shards(value),
    None => 16,
};

/// Parses `BRAP_SHARDS` during compilation; a bad value is a compile error.
const fn parse_shards(value: &str) -> usize {
    let digits = value.as_bytes();
    assert!(!digits.is_empty(), "BRAP_SHARDS must be a positive integer");
    let mut shards = 0_usize;
    let mut index = 0;
    while index < digits.len() {
        assert!(
            digits[index].is_ascii_digit(),
            "BRAP_SHARDS must be a positive integer"
        );
        shards = shards * 10 + (digits[index] - b'0') as usize;
        index += 1;
    }
    assert!(shards > 0, "BRAP_SHARDS must be a positive integer");
    shards
}

/// One copy of a histogram's bins. The alignment keeps neighbouring shards
/// off each other's cache lines, including adjacent-line prefetch.
#[repr(C, align(128))]
#[derive(Debug)]
struct Shard<const BINS: usize>([AtomicU64; BINS]);

/// Atomic bins spread over [`SHARDS`] copies. Each thread increments only its
/// own shard; draining swaps out and sums every shard.
#[derive(Debug)]
struct ShardedBins<const BINS: usize> {
    shards: [Shard<BINS>; SHARDS],
}

impl<const BINS: usize> ShardedBins<BINS> {
    const fn new() -> Self {
        Self {
            shards: [const { Shard([const { AtomicU64::new(0) }; BINS]) }; SHARDS],
        }
    }

    #[inline(always)]
    fn add(&self, index: usize, value: u64) {
        self.shards[shard_index()].0[index].fetch_add(value, Ordering::Relaxed);
    }

    // PERF: warming touches every shard, so each warmed site commits
    // SHARDS times its histogram (125 KiB for a fine site) even when only a
    // few threads ever record into it.
    fn touch_pages(&self) {
        const U64S_PER_PAGE: usize = 4096 / mem::size_of::<AtomicU64>();
        for Shard(bins) in &self.shards {
            for counter in bins.iter().step_by(U64S_PER_PAGE) {
                counter.fetch_add(0, Ordering::Relaxed);
            }
            if let Some(last) = bins.last() {
                last.fetch_add(0, Ordering::Relaxed);
            }
        }
    }

    fn take(&self) -> Option<(u64, Vec<u64>)> {
        let mut counters = self.shards.iter().flat_map(|Shard(bins)| bins);
        if !counters.any(|counter| counter.load(Ordering::Relaxed) != 0) {
            return None;
        }

        let mut sample_count = 0_u64;
        let mut snapshot = vec![0_u64; BINS];
        for Shard(bins) in &self.shards {
            for (total, counter) in snapshot.iter_mut().zip(bins) {
                // A racing first sample stays in the live bin for the next
                // interval; avoid a locked write to the overwhelmingly common
                // empty bins.
                if counter.load(Ordering::Relaxed) != 0 {
                    let samples = counter.swap(0, Ordering::Relaxed);
                    *total = total.saturating_add(samples);
                    sample_count = sample_count.saturating_add(samples);
                }
            }
        }
        (sample_count != 0).then_some((sample_count, snapshot))
    }
}

/// This thread's shard, assigned round-robin on its first shared record.
/// Without sharding this is a constant and touches no thread-local state.
#[inline(always)]
fn shard_index() -> usize {
    if SHARDS == 1 {
        return 0;
    }
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    thread_local! {
        static SHARD: usize = NEXT.fetch_add(1, Ordering::Relaxed) % SHARDS;
    }
    SHARD.with(|shard| *shard)
}

fn warm_counter<T: Copy>(counter: &mut T) {
    // SAFETY: `counter` is exclusively borrowed. The volatile read/write pair
    // preserves its value while forcing the backing TLS page writable before
    // the owner enters its hot loop.
    unsafe {
        let value = core::ptr::read_volatile(counter);
        core::ptr::write_volatile(counter, value);
    }
}

#[cfg(test)]
mod tests {
    use super::{Histogram, Log2Histogram, MantissaHistogram};

    #[test]
    fn log2_histogram_records_expected_bins() {
        let mut histogram = Log2Histogram::default();
        for value in [0, 1, 2, 3, 4, 7, 8, u64::MAX] {
            histogram.record(value);
        }
        assert_eq!(histogram.sample_count(), 8);
        let bins = histogram.take_bins();
        assert_eq!(bins[0], 2);
        assert_eq!(bins[1], 2);
        assert_eq!(bins[2], 2);
        assert_eq!(bins[3], 1);
        assert_eq!(bins[63], 1);
        let snapshot = super::histogram::Snapshot {
            layout: super::histogram::Layout::Log2,
            sample_count: 8,
            bins,
        };
        assert_eq!(snapshot.percentile_cycles(50), 3);
        assert_eq!(snapshot.percentile_cycles(100), 3_u64 << 62);

        type Fine4 = MantissaHistogram<976, 4>;
        let mut histogram = Fine4::default();
        for value in [0, 1, 15, 16, 17, 31, 32, 33, u64::MAX] {
            histogram.record(value);
        }
        assert_eq!(histogram.sample_count(), 9);
        let bins = histogram.take_bins();
        assert_eq!(bins[0], 1);
        assert_eq!(bins[1], 1);
        assert_eq!(bins[15], 1);
        assert_eq!(bins[16], 1);
        assert_eq!(bins[17], 1);
        assert_eq!(bins[31], 1);
        assert_eq!(bins[32], 2);
        assert_eq!(bins[975], 1);
        let snapshot = super::histogram::Snapshot {
            layout: super::histogram::Layout::Mantissa { sub_bucket_bits: 4 },
            sample_count: 9,
            bins,
        };
        assert_eq!(snapshot.percentile_cycles(50), 17);
        assert_eq!(
            snapshot.percentile_cycles(100),
            (31_u64 << 59) + (1_u64 << 58)
        );
    }
}
