//! Measures whole functions, splits one into stages with `checkpoint!`, and
//! drains the scope by hand.
//!
//! ```sh
//! cargo run -p brap-examples --example measure
//! ```

const SCOPE: &str = "brap-example-measure";

/// Without `name`, the metric is named after the function's path.
#[brap::measure(scope = SCOPE)]
fn example_unnamed(input: &str) -> usize {
    input.split_whitespace().count()
}

/// Each checkpoint records the cycles since the previous one. A call that
/// returns early never reaches later checkpoints, so `example-stage-2` sees
/// fewer samples than `example-stage-1`.
#[brap::measure_fine(name = "example-total", scope = SCOPE)]
fn example(input: &str) -> Result<u64, std::num::ParseIntError> {
    let trimmed = input.trim();
    brap::checkpoint!("example-stage-1");
    let value = trimmed.parse()?;
    brap::checkpoint!("example-stage-2");
    Ok(value)
}

fn main() {
    for input in ["1", " 22 ", "333", "not a number"] {
        example_unnamed(input);
        let _ = example(input);
    }

    for snapshot in brap::access_scope(SCOPE) {
        brap::log_snapshot(snapshot);
    }
}
