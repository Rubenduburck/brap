use std::io::{self, Write};

use crate::{
    ValueRef,
    clock::{calibrate_ns_per_cycle, cycles_to_micros},
    metric,
};

#[derive(Clone, Copy, Debug)]
pub struct Field<'a> {
    pub name: &'static str,
    pub value: FieldValue<'a>,
}

impl<'a> Field<'a> {
    pub const fn u64(name: &'static str, value: u64) -> Self {
        Self {
            name,
            value: FieldValue::U64(value),
        }
    }

    pub const fn bool(name: &'static str, value: bool) -> Self {
        Self {
            name,
            value: FieldValue::Bool(value),
        }
    }

    pub const fn str(name: &'static str, value: &'a str) -> Self {
        Self {
            name,
            value: FieldValue::Str(value),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum FieldValue<'a> {
    U64(u64),
    Bool(bool),
    Str(&'a str),
}

pub fn write_set(
    writer: &mut impl Write,
    set: &impl metric::Set,
    fields: &[Field<'_>],
) -> io::Result<()> {
    let mut result = Ok(());
    metric::Set::visit(set, &mut |metric| {
        if result.is_ok() {
            result = write_metric(writer, metric, fields);
        }
    });
    result
}

pub fn log_set(set: &impl metric::Set, fields: &[Field<'_>]) -> io::Result<()> {
    let stdout = io::stdout();
    write_set(&mut stdout.lock(), set, fields)
}

pub fn log_metric(metric: metric::Ref<'_>, fields: &[Field<'_>]) -> io::Result<()> {
    let stdout = io::stdout();
    write_metric(&mut stdout.lock(), metric, fields)
}

pub fn write_metric(
    writer: &mut impl Write,
    metric: metric::Ref<'_>,
    fields: &[Field<'_>],
) -> io::Result<()> {
    if matches!(metric.value, ValueRef::Histogram(_)) {
        calibrate_ns_per_cycle();
    }
    write!(writer, "brap kind={} scope=", metric.value.kind())?;
    write_logfmt_str(writer, metric.scope)?;
    write!(writer, " metric=")?;
    write_logfmt_str(writer, metric.name)?;
    match metric.value {
        ValueRef::Histogram(histogram) => {
            let min_cycles = histogram.percentile_cycles(0);
            let p50_cycles = histogram.percentile_cycles(50);
            let p95_cycles = histogram.percentile_cycles(95);
            let p99_cycles = histogram.percentile_cycles(99);
            let max_cycles = histogram.percentile_cycles(100);
            write!(
                writer,
                " count={} min_cycles={min_cycles} p50_cycles={p50_cycles} p95_cycles={p95_cycles} p99_cycles={p99_cycles} max_cycles={max_cycles} min_us={} p50_us={} p95_us={} p99_us={} max_us={}",
                histogram.sample_count,
                cycles_to_micros(min_cycles),
                cycles_to_micros(p50_cycles),
                cycles_to_micros(p95_cycles),
                cycles_to_micros(p99_cycles),
                cycles_to_micros(max_cycles),
            )?;
        }
        ValueRef::Counter(value) | ValueRef::Gauge(value) => {
            write!(writer, " value={value}")?;
        }
        ValueRef::Summary(value) => {
            write!(
                writer,
                " count={} sum={} min={} max={}",
                value.count, value.sum, value.min, value.max,
            )?;
        }
    }
    for field in fields {
        write!(writer, " {}=", field.name)?;
        match field.value {
            FieldValue::U64(value) => write!(writer, "{value}")?,
            FieldValue::Bool(value) => write!(writer, "{value}")?,
            FieldValue::Str(value) => write_logfmt_str(writer, value)?,
        }
    }
    writeln!(writer)
}

fn write_logfmt_str(writer: &mut impl Write, value: &str) -> io::Result<()> {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
    {
        return writer.write_all(value.as_bytes());
    }
    writer.write_all(b"\"")?;
    for character in value.chars() {
        match character {
            '\\' => writer.write_all(b"\\\\")?,
            '"' => writer.write_all(b"\\\"")?,
            '\n' => writer.write_all(b"\\n")?,
            '\r' => writer.write_all(b"\\r")?,
            character => write!(writer, "{character}")?,
        }
    }
    writer.write_all(b"\"")
}

#[cfg(test)]
mod tests {
    use super::{Field, write_set};
    use crate::{Counter, Gauge, Log2Histogram, Summary};

    #[derive(Default, crate::MetricSet)]
    #[brap(scope = "output-test")]
    struct TestMetrics {
        calls: Counter,
        depth: Gauge,
        values: Summary,
        latency: Log2Histogram,
    }

    #[test]
    fn writes_one_schema_for_every_metric_kind() {
        let mut metrics = TestMetrics::default();
        metrics.calls.increment();
        metrics.calls.add(2);
        metrics.depth.set(7);
        metrics.values.record(11);
        metrics.values.record(19);
        metrics.latency.record(16);

        let mut output = Vec::new();
        write_set(
            &mut output,
            &metrics,
            &[Field::u64("example_field", 42), Field::str("example_label", "example label")],
        )
        .unwrap();
        assert!(crate::clock::is_calibrated());
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("kind=counter scope=output-test metric=calls value=3"));
        assert!(output.contains("kind=gauge scope=output-test metric=depth value=7"));
        assert!(
            output.contains(
                "kind=summary scope=output-test metric=values count=2 sum=30 min=11 max=19"
            )
        );
        assert!(output.contains("kind=histogram scope=output-test metric=latency count=1"));
        assert!(output.contains(" min_cycles="));
        assert!(output.contains(" min_us="));
        assert!(output.contains(" example_field=42 example_label=\"example label\""));
    }
}
