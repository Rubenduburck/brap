use crate::HistogramBins;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Log2,
    Mantissa { sub_bucket_bits: u8 },
    /// A `count!` site: one bin holding the total, not a distribution.
    Count,
}

#[derive(Clone, Copy, Debug)]
pub struct Ref<'a> {
    pub layout: Layout,
    pub bins: HistogramBins<'a>,
    pub sample_count: u64,
}

impl Ref<'_> {
    pub fn percentile_cycles(self, percentile: u64) -> u64 {
        if self.sample_count == 0 {
            return 0;
        }
        let target = self
            .sample_count
            .saturating_mul(percentile.min(100))
            .div_ceil(100)
            .max(1);
        let mut cumulative = 0_u64;
        for index in 0..self.bins.len() {
            cumulative = cumulative.saturating_add(self.bins.get(index));
            if cumulative >= target {
                return bucket_midpoint_cycles(self.layout, index);
            }
        }
        0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub layout: Layout,
    pub sample_count: u64,
    pub bins: Vec<u64>,
}

impl Snapshot {
    pub fn as_ref(&self) -> Ref<'_> {
        Ref {
            layout: self.layout,
            sample_count: self.sample_count,
            bins: HistogramBins::U64(&self.bins),
        }
    }

    /// Returns an approximate percentile in cycles using the selected
    /// histogram's bucket midpoint. This is cold-side work.
    pub fn percentile_cycles(&self, percentile: u64) -> u64 {
        self.as_ref().percentile_cycles(percentile)
    }
}

fn bucket_midpoint_cycles(layout: Layout, index: usize) -> u64 {
    match layout {
        Layout::Count => 0,
        Layout::Log2 => {
            if index == 0 {
                1
            } else {
                (1_u64 << index).saturating_add(1_u64 << (index - 1))
            }
        }
        Layout::Mantissa { sub_bucket_bits } => {
            let sub_bucket_bits = usize::from(sub_bucket_bits);
            let sub_buckets = 1usize << sub_bucket_bits;
            if index < sub_buckets {
                return index.max(1) as u64;
            }
            let adjusted = index - sub_buckets;
            let exponent = sub_bucket_bits + adjusted / sub_buckets;
            let sub_bucket = adjusted % sub_buckets;
            let width = 1_u64 << (exponent - sub_bucket_bits);
            let lower = (1_u64 << exponent) + sub_bucket as u64 * width;
            lower + width / 2
        }
    }
}
