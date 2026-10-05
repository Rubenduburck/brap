//! Runs the background consumer while several threads record into one shared
//! site. Stopping the consumer flushes the final partial interval.
//!
//! ```sh
//! cargo run -p brap-examples --example consumer
//! ```

use std::time::Duration;

const SCOPE: &str = "brap-example-consumer";

#[brap::measure_fine(name = "example-work", scope = SCOPE)]
fn example_work(id: u64) {
    std::thread::sleep(Duration::from_micros(id % 50));
}

fn main() {
    // Without `.consume`, snapshots are logged to stdout in logfmt.
    let consumer = brap::consumer::build()
        .scope(SCOPE)
        .every(Duration::from_millis(100))
        .thread_name("brap-example-consumer")
        .consume(|snapshot| {
            println!(
                "{}: samples={} p50_cycles={} p99_cycles={}",
                snapshot.name,
                snapshot.histogram.sample_count,
                snapshot.histogram.percentile_cycles(50),
                snapshot.histogram.percentile_cycles(99),
            );
        })
        .spawn()
        .expect("start telemetry consumer");

    let workers: Vec<_> = (0..4)
        .map(|_| std::thread::spawn(|| (0..5_000).for_each(example_work)))
        .collect();
    for worker in workers {
        worker.join().expect("worker thread");
    }
    consumer.stop().expect("stop telemetry consumer");

    for status in brap::consumption_statuses() {
        println!(
            "{} consumed {} samples in {} drains",
            status.name, status.consumed_samples, status.consumption_calls,
        );
    }
}
