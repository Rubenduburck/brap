use std::{
    sync::{
        Once,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

static NS_PER_CYCLE_Q32: AtomicU64 = AtomicU64::new(0);
static CALIBRATION: Once = Once::new();

#[inline(always)]
pub fn now_cycles() -> u64 {
    // Deliberately the fastest cycle read, without an ordering fence.
    unsafe { core::arch::x86_64::_rdtsc() }
}

#[inline(always)]
pub fn cycles_to_nanos(cycles: u64) -> u64 {
    let nanos_per_cycle = NS_PER_CYCLE_Q32.load(Ordering::Relaxed);
    ((u128::from(cycles) * u128::from(nanos_per_cycle)) >> 32) as u64
}

#[inline(always)]
pub fn cycles_to_micros(cycles: u64) -> u64 {
    cycles_to_nanos(cycles) / 1_000
}

pub fn is_calibrated() -> bool {
    NS_PER_CYCLE_Q32.load(Ordering::Relaxed) != 0
}

pub fn calibrate_ns_per_cycle() {
    CALIBRATION.call_once(|| {
        const CALIBRATION_TIME: Duration = Duration::from_millis(100);

        let started_at = Instant::now();
        let started_cycles = now_cycles();
        std::thread::sleep(CALIBRATION_TIME);
        let elapsed_cycles = now_cycles().wrapping_sub(started_cycles).max(1);
        let elapsed_nanos = started_at.elapsed().as_nanos();
        let nanos_per_cycle = (elapsed_nanos << 32) / u128::from(elapsed_cycles);
        NS_PER_CYCLE_Q32.store(nanos_per_cycle as u64, Ordering::Relaxed);
    });
}
