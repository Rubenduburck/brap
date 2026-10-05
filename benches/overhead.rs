//! Cost of each recording operation, called back to back on 1 and on 16
//! threads at once. Every bench calls an `#[inline(never)]` function; the
//! uninstrumented `baseline` is subtracted when the results are plotted.
//!
//! ```sh
//! cargo bench --bench overhead
//! ```

use std::{
    hint::black_box,
    sync::Barrier,
    time::{Duration, Instant},
};

use criterion::{BenchmarkId, Criterion};

const SCOPE: &str = "brap-bench";
const THREADS: [usize; 2] = [1, 16];

#[inline(never)]
fn baseline(value: u64) -> u64 {
    black_box(value)
}

#[inline(never)]
#[brap::measure(scope = SCOPE)]
fn measure(value: u64) -> u64 {
    black_box(value)
}

#[inline(never)]
#[brap::measure_fine(scope = SCOPE)]
fn measure_fine(value: u64) -> u64 {
    black_box(value)
}

#[inline(never)]
#[brap::measure(scope = SCOPE, storage = "tls")]
fn measure_tls(value: u64) -> u64 {
    black_box(value)
}

#[inline(never)]
#[brap::measure_fine(scope = SCOPE, storage = "tls")]
fn measure_fine_tls(value: u64) -> u64 {
    black_box(value)
}

#[inline(never)]
fn record_log2(value: u64) -> u64 {
    brap::record_log2!(SCOPE, "record_log2", value);
    value
}

#[inline(never)]
fn record_log2_tls(value: u64) -> u64 {
    brap::record_log2!(SCOPE, "record_log2_tls", value, storage = "tls");
    value
}

#[inline(never)]
fn count(value: u64) -> u64 {
    brap::count!(SCOPE, "count");
    value
}

#[inline(never)]
fn count_tls(value: u64) -> u64 {
    brap::count!(SCOPE, "count_tls", storage = "tls");
    value
}

#[inline(never)]
fn stamp(value: u64) -> u64 {
    black_box(brap::stamp!()) ^ value
}

/// Runs `call` `iters` times on each of `threads` threads at once and returns
/// the wall time from the common start: per-thread time per call while every
/// thread records. Spawning and TLS warm-up happen before the start.
fn concurrent(threads: usize, iters: u64, call: fn(u64) -> u64) -> Duration {
    let barrier = Barrier::new(threads + 1);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let _owner = brap::TlsOwnerGuard::enter(SCOPE).warm();
                barrier.wait();
                for value in 0..iters {
                    call(black_box(value));
                }
            });
        }
        barrier.wait();
        Instant::now()
    })
    .elapsed()
}

fn bench(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("overhead");
    group
        .sample_size(30)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    let operations = [
        ("baseline", baseline as fn(u64) -> u64),
        ("stamp!", stamp),
        ("count!", count),
        ("count! tls", count_tls),
        ("record_log2!", record_log2),
        ("record_log2! tls", record_log2_tls),
        ("measure", measure),
        ("measure_fine", measure_fine),
        ("measure tls", measure_tls),
        ("measure_fine tls", measure_fine_tls),
    ];
    for (name, call) in operations {
        for threads in THREADS {
            group.bench_function(BenchmarkId::new(name, threads), |b| {
                b.iter_custom(|iters| concurrent(threads, iters, call))
            });
        }
    }
    group.finish();
}

criterion::criterion_group!(benches, bench);
criterion::criterion_main!(benches);
