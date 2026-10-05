#![cfg(feature = "brap")]

use brap::{MetricStorage, TlsOwnerGuard, access_scope, consumption_statuses, histogram};

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(value) => return value,
            std::task::Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[brap::measure(name = "test_log2", scope = "measure-test", histogram = "log2")]
fn measured_log2(value: u64) -> u64 {
    value + 1
}

#[brap::measure_fine(name = "test_fine", scope = "measure-test")]
fn measured_fine(value: u64) -> Result<u64, &'static str> {
    if value == 0 {
        return Err("zero");
    }
    Ok(value + 1)
}

#[brap::measure(
    name = "test_fresh_log2",
    scope = "fresh-measure-test",
    storage = "tls"
)]
fn measured_fresh_log2(value: u64) -> u64 {
    value + 1
}

struct Calculator(u64);

impl Calculator {
    #[brap::measure_fine(
        name = "test_method",
        scope = "method-measure-test",
        precision = 5,
        storage = "tls"
    )]
    fn multiply(&self, value: u64) -> u64 {
        let product = self.0 * value;
        brap::checkpoint!("test_method_checkpoint");
        product
    }
}

fn record_explicit_deltas(log2_cycles: u64, fine_cycles: u64) {
    brap::record_log2!("explicit-measure-test", "test_explicit_log2", log2_cycles,);
    brap::record_fine!(
        "explicit-measure-test",
        "test_explicit_fine",
        fine_cycles,
        precision = 3,
    );
}

fn record_with_default_scope(cycles: u64) {
    brap::record_fine!("test_default_scope", cycles);
}

#[brap::measure]
fn measured_with_defaults(value: u64) -> u64 {
    value + 1
}

#[brap::measure(name = "test_cross_thread", scope = "cross-thread-test")]
fn measured_cross_thread(value: u64) -> u64 {
    value + 1
}

#[brap::measure(name = "test_consumer", scope = "consumer-test")]
fn measured_for_consumer(value: u64) -> u64 {
    value + 1
}

#[brap::measure(name = "test_final_flush", scope = "final-flush-test")]
fn measured_for_final_flush(value: u64) -> u64 {
    value + 1
}

#[brap::measure_fine(
    name = "test_checkpoint_total",
    scope = "checkpoint-test",
    precision = 3
)]
fn measured_with_checkpoints(value: u64) -> Result<u64, &'static str> {
    let value = std::hint::black_box(value + 1);
    brap::checkpoint!("test_checkpoint_first");
    if value == 1 {
        return Err("one");
    }
    let value = std::hint::black_box(value * 2);
    brap::checkpoint!("test_checkpoint_second");
    Ok(value)
}

#[brap::measure_fine(name = "test_async_total", scope = "async-measure-test")]
async fn measured_async_with_checkpoints(value: u64) -> Result<u64, &'static str> {
    let value = std::future::ready(value + 1).await;
    brap::checkpoint!("test_async_first");
    if value == 1 {
        return Err("one");
    }
    let value = std::future::ready(value * 2).await;
    brap::checkpoint!("test_async_second");
    Ok(value)
}

#[test]
fn attribute_declares_records_and_exposes_metrics() {
    assert_eq!(measured_log2(3), 4);
    assert_eq!(measured_fine(0), Err("zero"));
    assert_eq!(measured_fine(4), Ok(5));

    let mut snapshots = access_scope("measure-test");
    snapshots.sort_by_key(|snapshot| snapshot.name);
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].name, "test_fine");
    assert_eq!(snapshots[0].storage, MetricStorage::Atomic);
    assert_eq!(snapshots[0].histogram.sample_count, 2);
    assert_eq!(
        snapshots[0].histogram.layout,
        histogram::Layout::Mantissa { sub_bucket_bits: 4 }
    );
    assert_eq!(snapshots[1].name, "test_log2");
    assert_eq!(snapshots[1].histogram.sample_count, 1);
    assert_eq!(snapshots[1].histogram.layout, histogram::Layout::Log2);

    let statuses = consumption_statuses();
    let log2 = statuses
        .iter()
        .find(|status| status.name == "test_log2")
        .expect("log2 descriptor");
    assert_eq!(log2.fresh_accesses, 0);
    assert_eq!(log2.consumption_calls, 1);
    assert_eq!(log2.consumed_samples, 1);
    assert!(log2.was_consumed());
    assert!(!log2.was_used_before_warm());

    let _owner = TlsOwnerGuard::enter("method-measure-test").warm();
    assert_eq!(Calculator(7).multiply(6), 42);

    let snapshots = access_scope("method-measure-test");
    let snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.name == "test_method")
        .expect("method snapshot");
    assert_eq!(snapshot.histogram.sample_count, 1);
    assert_eq!(snapshot.storage, MetricStorage::Tls);
    assert_eq!(
        snapshot.histogram.layout,
        histogram::Layout::Mantissa { sub_bucket_bits: 5 }
    );
    let checkpoint = snapshots
        .iter()
        .find(|snapshot| snapshot.name == "test_method_checkpoint")
        .expect("method checkpoint snapshot");
    assert_eq!(checkpoint.histogram.sample_count, 1);
    assert_eq!(checkpoint.storage, MetricStorage::Tls);
    assert_eq!(
        checkpoint.histogram.layout,
        histogram::Layout::Mantissa { sub_bucket_bits: 5 }
    );

    record_explicit_deltas(64, 123);

    let mut snapshots = access_scope("explicit-measure-test");
    snapshots.sort_by_key(|snapshot| snapshot.name);
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].name, "test_explicit_fine");
    assert_eq!(snapshots[0].histogram.sample_count, 1);
    assert_eq!(
        snapshots[0].histogram.layout,
        histogram::Layout::Mantissa { sub_bucket_bits: 3 }
    );
    assert_eq!(snapshots[1].name, "test_explicit_log2");
    assert_eq!(snapshots[1].histogram.sample_count, 1);

    record_with_default_scope(71);
    assert_eq!(measured_with_defaults(10), 11);

    let snapshots = access_scope(module_path!());
    let snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.name == "test_default_scope")
        .expect("default-scope snapshot");
    assert_eq!(snapshot.histogram.sample_count, 1);
    assert_eq!(
        snapshot.histogram.layout,
        histogram::Layout::Mantissa { sub_bucket_bits: 4 }
    );
    let measured = snapshots
        .iter()
        .find(|snapshot| snapshot.name.ends_with("::measured_with_defaults"))
        .expect("default-name snapshot");
    assert_eq!(measured.scope, module_path!());
    assert_eq!(measured.storage, MetricStorage::Atomic);
    assert_eq!(measured.histogram.layout, histogram::Layout::Log2);
}

#[test]
fn access_before_owner_warm_is_detected_without_losing_the_sample() {
    let snapshot = std::thread::spawn(|| {
        assert_eq!(measured_fresh_log2(9), 10);
        access_scope("fresh-measure-test")
            .into_iter()
            .find(|snapshot| snapshot.name == "test_fresh_log2")
            .expect("fresh-thread snapshot")
    })
    .join()
    .expect("fresh thread");

    assert_eq!(snapshot.histogram.sample_count, 1);
    assert_eq!(snapshot.storage, MetricStorage::Tls);
    let statuses = consumption_statuses();
    let log2 = statuses
        .iter()
        .find(|status| status.name == "test_fresh_log2")
        .expect("log2 descriptor");
    assert_eq!(log2.fresh_accesses, 1);
    assert_eq!(log2.consumption_calls, 1);
    assert_eq!(log2.consumed_samples, 1);
    assert!(log2.was_consumed());
    assert!(log2.was_used_before_warm());
}

#[test]
fn atomic_sites_can_be_consumed_from_another_thread() {
    let producers: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                for value in 0..100 {
                    assert_eq!(measured_cross_thread(value), value + 1);
                }
            })
        })
        .collect();
    for producer in producers {
        producer.join().expect("producer thread");
    }

    let snapshot = access_scope("cross-thread-test")
        .into_iter()
        .find(|snapshot| snapshot.name == "test_cross_thread")
        .expect("cross-thread snapshot");
    assert_eq!(snapshot.storage, MetricStorage::Atomic);
    assert_eq!(snapshot.histogram.sample_count, 400);
}

#[test]
fn checkpoints_record_consecutive_laps_and_inherit_the_parent_site() {
    assert_eq!(measured_with_checkpoints(0), Err("one"));
    assert_eq!(measured_with_checkpoints(4), Ok(10));

    let snapshots = access_scope("checkpoint-test");
    let total = snapshots
        .iter()
        .find(|snapshot| snapshot.name == "test_checkpoint_total")
        .expect("total metric");
    let first = snapshots
        .iter()
        .find(|snapshot| snapshot.name == "test_checkpoint_first")
        .expect("first checkpoint metric");
    let second = snapshots
        .iter()
        .find(|snapshot| snapshot.name == "test_checkpoint_second")
        .expect("second checkpoint metric");

    assert_eq!(total.histogram.sample_count, 2);
    assert_eq!(first.histogram.sample_count, 2);
    assert_eq!(second.histogram.sample_count, 1);
    for snapshot in [total, first, second] {
        assert_eq!(snapshot.scope, "checkpoint-test");
        assert_eq!(snapshot.storage, MetricStorage::Atomic);
        assert_eq!(
            snapshot.histogram.layout,
            histogram::Layout::Mantissa { sub_bucket_bits: 3 }
        );
    }
}

#[test]
fn async_attributes_measure_the_polled_operation_and_checkpoints() {
    assert_eq!(block_on(measured_async_with_checkpoints(0)), Err("one"));
    assert_eq!(block_on(measured_async_with_checkpoints(4)), Ok(10));

    let snapshots = access_scope("async-measure-test");
    let total = snapshots
        .iter()
        .find(|snapshot| snapshot.name == "test_async_total")
        .expect("async total metric");
    let first = snapshots
        .iter()
        .find(|snapshot| snapshot.name == "test_async_first")
        .expect("async first checkpoint metric");
    let second = snapshots
        .iter()
        .find(|snapshot| snapshot.name == "test_async_second")
        .expect("async second checkpoint metric");
    assert_eq!(total.histogram.sample_count, 2);
    assert_eq!(first.histogram.sample_count, 2);
    assert_eq!(second.histogram.sample_count, 1);
}

#[test]
fn periodic_consumer_discovers_and_invokes_configured_function() {
    let (send, receive) = std::sync::mpsc::channel();
    let consumer = brap::consumer::build()
        .scope("consumer-test")
        .every(std::time::Duration::from_millis(5))
        .consume(move |snapshot| send.send(snapshot).expect("consumer receiver is alive"))
        .spawn()
        .expect("spawn telemetry consumer");

    assert_eq!(measured_for_consumer(8), 9);
    let snapshot = receive
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("periodic snapshot");
    assert_eq!(snapshot.name, "test_consumer");
    assert_eq!(snapshot.histogram.sample_count, 1);
    consumer.stop().expect("stop telemetry consumer");
    let status = consumption_statuses()
        .into_iter()
        .find(|status| status.name == "test_consumer")
        .expect("consumer metric status");
    assert!(status.was_consumed());
    assert_eq!(status.consumed_samples, 1);
}

#[test]
fn stopping_consumer_flushes_the_final_partial_interval() {
    let (send, receive) = std::sync::mpsc::channel();
    let consumer = brap::consumer::build()
        .scope("final-flush-test")
        .every(std::time::Duration::from_secs(60 * 60))
        .consume(move |snapshot| send.send(snapshot).expect("consumer receiver is alive"))
        .spawn()
        .expect("spawn telemetry consumer");

    assert_eq!(measured_for_final_flush(8), 9);
    consumer.stop().expect("stop telemetry consumer");
    let snapshot = receive
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("final telemetry snapshot");
    assert_eq!(snapshot.name, "test_final_flush");
    assert_eq!(snapshot.histogram.sample_count, 1);
}

#[test]
fn count_sites_total_events_and_log_as_counters() {
    for batch in [1, 2, 3] {
        brap::count!("count-test", "test_count_events");
        brap::count!("count-test", "test_count_items", batch);
    }
    let _owner = TlsOwnerGuard::enter("count-tls-test").warm();
    brap::count!("count-tls-test", "test_count_tls", 5, storage = "tls");

    let mut snapshots = access_scope("count-test");
    snapshots.sort_by_key(|snapshot| snapshot.name);
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].name, "test_count_events");
    assert_eq!(snapshots[0].histogram.layout, histogram::Layout::Count);
    assert_eq!(snapshots[0].histogram.sample_count, 3);
    assert_eq!(snapshots[1].name, "test_count_items");
    assert_eq!(snapshots[1].histogram.sample_count, 6);
    assert!(access_scope("count-test").is_empty());

    let mut output = Vec::new();
    brap::write_metric(&mut output, snapshots[1].as_ref(), &[]).expect("write count");
    assert_eq!(
        String::from_utf8(output).expect("utf-8 output"),
        "brap kind=counter scope=count-test metric=test_count_items value=6\n"
    );

    let tls = access_scope("count-tls-test");
    assert_eq!(tls.len(), 1);
    assert_eq!(tls[0].storage, MetricStorage::Tls);
    assert_eq!(tls[0].histogram.sample_count, 5);
}
