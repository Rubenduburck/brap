//! Records explicitly delimited spans with `stamp!` and `record_*!`, counts
//! events with `count!`, and gates one span behind the `example` compile set.
//!
//! ```sh
//! cargo run -p brap-examples --example record
//! # Drops the `example` set: `example-set-span` is neither timed nor recorded.
//! cargo run -p brap-examples --example record --no-default-features --features brap
//! ```

const SCOPE: &str = "brap-example-record";

fn main() {
    let mut items = Vec::with_capacity(64);
    for value in 0..10_000_u64 {
        let started = brap::stamp!();
        items.push(value);
        brap::count!(SCOPE, "example-items");
        brap::record_log2!(SCOPE, "example-span", brap::now_cycles() - started);

        if items.len() == items.capacity() {
            // A disabled set compiles `stamp!` to 0 and skips the cycles
            // expression entirely, so no clock is read.
            let set_started = brap::stamp!(set = "example");
            brap::count!(SCOPE, "example-drained", items.len() as u64);
            std::hint::black_box(items.drain(..).sum::<u64>());
            brap::record_fine!(
                SCOPE,
                "example-set-span",
                brap::now_cycles() - set_started,
                set = "example",
            );
        }
    }

    for snapshot in brap::access_scope(SCOPE) {
        brap::log_snapshot(snapshot);
    }
}
