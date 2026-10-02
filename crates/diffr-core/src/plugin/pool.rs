//! A per-plugin deferred-work bound shared by every file. Independent
//! instances are opt-in; a serial plugin keeps one instance for its whole run.
use super::{host::Host, Runner};
use diffr_plugin_sdk::types;
use std::sync::{Condvar, Mutex};

pub(super) struct Pool {
    initial: Option<Mutex<Box<dyn Runner>>>,
    idle: Mutex<Vec<Box<dyn Runner>>>,
    available: Condvar,
}

impl Pool {
    #[cfg(test)]
    fn new(instances: Vec<Box<dyn Runner>>) -> Self {
        Self::with_initial(instances, None)
    }

    pub(super) fn with_initial(
        instances: Vec<Box<dyn Runner>>,
        initial: Option<Box<dyn Runner>>,
    ) -> Self {
        assert!(
            !instances.is_empty(),
            "a plugin needs at least one instance"
        );
        Self {
            initial: initial.map(Mutex::new),
            idle: Mutex::new(instances),
            available: Condvar::new(),
        }
    }

    // Fast presentation never waits for a pool full of network requests.
    // Serial plugins share their sole instance across both phases.
    fn initial<T>(&self, call: impl FnOnce(&dyn Runner) -> anyhow::Result<T>) -> anyhow::Result<T> {
        match &self.initial {
            Some(instance) => call(instance.lock().expect("initial plugin instance").as_ref()),
            None => self.with(call),
        }
    }

    fn with<T>(&self, call: impl FnOnce(&dyn Runner) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let mut idle = self.idle.lock().expect("instance pool");
        while idle.is_empty() {
            idle = self.available.wait(idle).expect("instance pool");
        }
        let lease = Lease {
            pool: self,
            instance: idle.pop(),
        };
        drop(idle);
        call(lease.instance.as_deref().expect("leased instance"))
    }
}

/// Return the slot on both errors and unwinding; never hold the queue lock
/// while executing a guest or waiting on its HTTP response.
struct Lease<'a> {
    pool: &'a Pool,
    instance: Option<Box<dyn Runner>>,
}
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.pool
            .idle
            .lock()
            .expect("instance pool")
            .push(self.instance.take().expect("leased instance"));
        self.pool.available.notify_one();
    }
}

impl Runner for Pool {
    fn queries(&self, host: Host) -> anyhow::Result<Vec<types::QuerySource>> {
        self.initial(|runner| runner.queries(host))
    }
    fn classify(&self, host: Host, file: &types::FileEntry) -> anyhow::Result<Vec<String>> {
        self.initial(|runner| runner.classify(host, file))
    }
    fn mutate(
        &self,
        host: Host,
        file: &types::FileEntry,
        sides: &types::SourceSides,
    ) -> anyhow::Result<Vec<types::Move>> {
        self.initial(|runner| runner.mutate(host, file, sides))
    }
    fn enrich(
        &self,
        host: Host,
        file: &types::FileEntry,
        sides: &types::SourceSides,
    ) -> anyhow::Result<Vec<types::Annotation>> {
        self.with(|runner| runner.enrich(host, file, sides))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    struct Empty;
    impl Runner for Empty {
        fn queries(&self, _: Host) -> anyhow::Result<Vec<types::QuerySource>> {
            Ok(vec![])
        }
        fn classify(&self, _: Host, _: &types::FileEntry) -> anyhow::Result<Vec<String>> {
            Ok(vec![])
        }
        fn mutate(
            &self,
            _: Host,
            _: &types::FileEntry,
            _: &types::SourceSides,
        ) -> anyhow::Result<Vec<types::Move>> {
            Ok(vec![])
        }
    }

    #[test]
    fn independent_calls_overlap_but_never_exceed_the_bound() {
        let pool = Pool::new(vec![Box::new(Empty), Box::new(Empty)]);
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (entered, arrivals) = mpsc::channel();
        std::thread::scope(|scope| {
            for _ in 0..6 {
                let (pool, active, peak, entered, gate) =
                    (&pool, &active, &peak, entered.clone(), gate.clone());
                scope.spawn(move || {
                    pool.with(|_| {
                        let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(count, Ordering::SeqCst);
                        entered.send(()).unwrap();
                        let (lock, ready) = &*gate;
                        let mut open = lock.lock().unwrap();
                        while !*open {
                            open = ready.wait(open).unwrap();
                        }
                        active.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .unwrap()
                });
            }
            let first = arrivals.recv_timeout(Duration::from_secs(5));
            let second = arrivals.recv_timeout(Duration::from_secs(5));
            // Always release the workers, even if an assertion will fail.
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();
            assert!(
                first.is_ok() && second.is_ok(),
                "two requests should enter before either returns"
            );
        });
        assert_eq!(peak.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn initial_presentation_does_not_wait_for_busy_enrichment() {
        let pool = Pool::with_initial(vec![Box::new(Empty)], Some(Box::new(Empty)));
        pool.with(|_| {
            // Holding the only enrichment lease must not block initial calls.
            assert_eq!(pool.initial(|_| Ok(7))?, 7);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn errors_return_the_slot_to_a_serial_pool() {
        let pool = Pool::new(vec![Box::new(Empty)]);
        assert!(pool
            .with::<()>(|_| anyhow::bail!("request failed"))
            .is_err());
        assert_eq!(pool.with(|_| Ok(42)).unwrap(), 42);
    }
}
