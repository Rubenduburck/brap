//! Owns metrics directly instead of declaring sites: a derived `MetricSet`
//! bundle and a `HistogramSet` indexed by a derived `MetricKey`. Both are plain
//! values the owner records into and emits on its own schedule.
//!
//! ```sh
//! cargo run -p brap-examples --example metric_set
//! ```

const SCOPE: &str = "brap-example-metric-set";

#[derive(Default, brap::MetricSet)]
#[brap(scope = SCOPE)]
struct ExampleMetrics {
    example_counter: brap::Counter,
    example_gauge: brap::Gauge,
    #[brap(name = "example-renamed-summary")]
    example_summary: brap::Summary,
    example_histogram: brap::FineHistogram,
}

/// Each variant names one histogram; `name` overrides the snake_case default.
#[derive(Clone, Copy, brap::MetricKey)]
#[brap(scope = SCOPE)]
enum ExampleStage {
    First,
    #[brap(name = "example-renamed-stage")]
    Second,
}

type ExampleStageHistograms = brap::HistogramSet<ExampleStage, brap::Log2Histogram, 2>;

fn main() {
    let mut metrics = ExampleMetrics::default();
    let mut stages = ExampleStageHistograms::default();

    for iteration in 0..100_u64 {
        let started = brap::now_cycles();
        metrics.example_counter.increment();
        metrics.example_gauge.set(iteration);
        metrics.example_summary.record(iteration % 17);

        let first_done = brap::now_cycles();
        stages.record(ExampleStage::First, first_done - started);
        let second_done = brap::now_cycles();
        stages.record(ExampleStage::Second, second_done - first_done);
        metrics.example_histogram.record(second_done - started);
    }

    let fields = [
        brap::Field::u64("example_field", 1),
        brap::Field::str("example_label", "brap-example"),
    ];
    brap::log_set(&metrics, &fields).expect("write example metrics");
    brap::log_set(&stages, &fields).expect("write example stage metrics");
}
