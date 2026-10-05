//! Uses thread-local storage for a single-owner hot loop. The owner thread
//! registers and warms its sites up front, and drains them itself.
//!
//! ```sh
//! cargo run -p brap-examples --example tls
//! ```

const SCOPE: &str = "brap-example-tls";

#[brap::measure(name = "example-hot-step", scope = SCOPE, storage = "tls")]
fn example_hot_step(value: u64) -> bool {
    std::hint::black_box(value.is_multiple_of(3))
}

fn main() {
    let warmed = std::thread::spawn(|| {
        let _owner = brap::TlsOwnerGuard::enter(SCOPE).warm();
        (0..100_000).for_each(|value| {
            example_hot_step(value);
        });
        brap::access_scope(SCOPE).into_iter().for_each(brap::log_snapshot);
    });
    warmed.join().expect("owner thread");

    // Recording before warm-up still works, but is reported as a fresh access.
    let cold = std::thread::spawn(|| {
        example_hot_step(7);
        brap::access_scope(SCOPE).into_iter().for_each(brap::log_snapshot);
    });
    cold.join().expect("cold thread");

    for status in brap::consumption_statuses() {
        println!(
            "{} used_before_warm={}",
            status.name,
            status.was_used_before_warm(),
        );
    }
}
