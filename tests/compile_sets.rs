use std::sync::atomic::{AtomicU64, Ordering};

static ATTRIBUTE_BODY_CALLS: AtomicU64 = AtomicU64::new(0);
static RECORD_EXPRESSIONS: AtomicU64 = AtomicU64::new(0);

#[brap::measure(
    name = "compile_set_attribute",
    scope = "compile-set-test",
    set = "test-set"
)]
fn measured_in_set(value: u64) -> u64 {
    ATTRIBUTE_BODY_CALLS.fetch_add(1, Ordering::Relaxed);
    brap::checkpoint!("compile_set_checkpoint");
    if value == 0 {
        return 7;
    }
    value + 1
}

fn record_in_set() {
    brap::record_log2!(
        "compile-set-test",
        "compile_set_record",
        {
            RECORD_EXPRESSIONS.fetch_add(1, Ordering::Relaxed);
            42
        },
        set = "test-set",
    );
    brap::count!(
        "compile-set-test",
        "compile_set_count",
        {
            RECORD_EXPRESSIONS.fetch_add(1, Ordering::Relaxed);
            2
        },
        set = "test-set",
    );
}

#[cfg(all(feature = "brap", feature = "brap-test-set"))]
#[test]
fn enabled_set_records_attributes_expressions_and_stamps() {
    assert_eq!(measured_in_set(0), 7);
    assert_eq!(measured_in_set(9), 10);
    record_in_set();
    assert_eq!(ATTRIBUTE_BODY_CALLS.load(Ordering::Relaxed), 2);
    assert_eq!(RECORD_EXPRESSIONS.load(Ordering::Relaxed), 2);
    assert_ne!(brap::stamp!(set = "test-set"), 0);

    let mut snapshots = brap::access_scope("compile-set-test");
    snapshots.sort_by_key(|snapshot| snapshot.name);
    assert_eq!(snapshots.len(), 4);
    assert_eq!(snapshots[0].name, "compile_set_attribute");
    assert_eq!(snapshots[0].histogram.sample_count, 2);
    assert_eq!(snapshots[1].name, "compile_set_checkpoint");
    assert_eq!(snapshots[1].histogram.sample_count, 2);
    assert_eq!(snapshots[2].name, "compile_set_count");
    assert_eq!(snapshots[2].histogram.sample_count, 2);
    assert_eq!(snapshots[3].name, "compile_set_record");
    assert_eq!(snapshots[3].histogram.sample_count, 1);
}

#[cfg(not(all(feature = "brap", feature = "brap-test-set")))]
#[test]
fn disabled_set_preserves_function_semantics_without_recording_work() {
    assert_eq!(measured_in_set(0), 7);
    assert_eq!(measured_in_set(9), 10);
    record_in_set();
    assert_eq!(ATTRIBUTE_BODY_CALLS.load(Ordering::Relaxed), 2);
    assert_eq!(RECORD_EXPRESSIONS.load(Ordering::Relaxed), 0);
    assert_eq!(brap::stamp!(set = "test-set"), 0);
    assert!(
        brap::consumption_statuses()
            .iter()
            .all(|status| !status.name.starts_with("compile_set_"))
    );
}
