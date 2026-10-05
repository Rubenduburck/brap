use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{MetricStorage, log_snapshot, metric};

/// Creates a consumer that automatically discovers shared atomic sites. It
/// logs every ten seconds by default; the returned builder configures its
/// interval, scope, and callback.
pub fn build() -> Builder {
    Builder::default()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub name: &'static str,
    pub scope: &'static str,
    pub storage: MetricStorage,
    pub fresh_accesses: u64,
    pub consumption_calls: u64,
    pub consumed_samples: u64,
}

impl Status {
    pub const fn was_consumed(&self) -> bool {
        self.consumption_calls != 0
    }

    pub const fn was_used_before_warm(&self) -> bool {
        self.fresh_accesses != 0
    }
}

type Consume = Box<dyn FnMut(metric::Snapshot) + Send + 'static>;

pub struct Builder {
    interval: Duration,
    scope: Option<&'static str>,
    thread_name: String,
    consume: Consume,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(10),
            scope: None,
            thread_name: "brap".to_owned(),
            consume: Box::new(log_snapshot),
        }
    }
}

impl Builder {
    pub fn every(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    pub fn scope(mut self, scope: &'static str) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn thread_name(mut self, name: impl Into<String>) -> Self {
        self.thread_name = name.into();
        self
    }

    pub fn consume(mut self, consume: impl FnMut(metric::Snapshot) + Send + 'static) -> Self {
        self.consume = Box::new(consume);
        self
    }

    pub fn spawn(mut self) -> io::Result<Handle> {
        if self.interval.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "telemetry consumer interval must be nonzero",
            ));
        }

        crate::warm_atomic(self.scope);

        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let interval = self.interval;
        let scope = self.scope;
        let join = thread::Builder::new()
            .name(self.thread_name)
            .spawn(move || {
                if !crate::clock::is_calibrated() {
                    crate::clock::calibrate_ns_per_cycle();
                }
                while !worker_stop.load(Ordering::Acquire) {
                    thread::park_timeout(interval);
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    crate::visit_atomic(scope, &mut self.consume);
                }
                crate::visit_atomic(scope, &mut self.consume);
            })?;
        Ok(Handle {
            stop,
            join: Some(join),
        })
    }
}

#[must_use = "dropping the consumer stops future telemetry collection"]
pub struct Handle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn stop(mut self) -> thread::Result<()> {
        self.signal_stop();
        self.join.take().expect("consumer worker is present").join()
    }

    fn signal_stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.as_ref() {
            join.thread().unpark();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.signal_stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
