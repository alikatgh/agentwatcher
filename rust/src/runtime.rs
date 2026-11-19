use crate::telemetry::Record;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistenceFailure {
    pub revision: u64,
    pub message: String,
}

/// A small, coalesced status snapshot; completion notifications cannot build up
/// while the UI is occupied. Revisions refer to submissions, not write attempts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PersistenceStatus {
    pub submitted_revision: u64,
    pub persisted_revision: u64,
    pub running_revision: Option<u64>,
    pub pending_revision: Option<u64>,
    pub failure: Option<PersistenceFailure>,
    pub closed: bool,
    pub stopped: bool,
}

impl PersistenceStatus {
    pub fn is_saved(&self) -> bool {
        self.persisted_revision == self.submitted_revision
    }

    pub fn is_failed(&self) -> bool {
        !self.is_saved()
            && self.running_revision.is_none()
            && self.pending_revision.is_none()
            && self.failure.is_some()
    }
}

struct PersistenceState<I> {
    latest: Option<Arc<I>>,
    status: PersistenceStatus,
}

/// One writer, one latest pending value, and no disk work under the mutex.
/// Unlike a refresh worker, failed values remain available for explicit retry
/// and dropping the owner drains accepted work instead of discarding it.
pub struct PersistenceWorker<I> {
    handle: PersistenceHandle<I>,
}

pub struct PersistenceHandle<I> {
    state: Arc<(Mutex<PersistenceState<I>>, Condvar)>,
}

impl<I> Clone for PersistenceHandle<I> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

impl<I: Send + Sync + 'static> PersistenceWorker<I> {
    pub fn new(mut operation: impl FnMut(&I) -> Result<(), String> + Send + 'static) -> Self {
        let handle = PersistenceHandle {
            state: Arc::new((
                Mutex::new(PersistenceState::<I> {
                    latest: None,
                    status: PersistenceStatus::default(),
                }),
                Condvar::new(),
            )),
        };
        let worker = handle.clone();
        let spawned = std::thread::Builder::new()
            .name("settings-writer".into())
            .spawn(move || {
                loop {
                    let (revision, value) = {
                        let (lock, wake) = &*worker.state;
                        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
                        while state.status.pending_revision.is_none() && !state.status.closed {
                            state = wake.wait(state).unwrap_or_else(|e| e.into_inner());
                        }
                        let Some(revision) = state.status.pending_revision.take() else {
                            state.status.stopped = true;
                            wake.notify_all();
                            break;
                        };
                        state.status.running_revision = Some(revision);
                        (revision, state.latest.as_ref().unwrap().clone())
                    };
                    // A panic must surface as a failed save, not strand quit forever.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        operation(&value)
                    }))
                    .unwrap_or_else(|_| Err("The settings writer stopped unexpectedly.".into()));
                    let (lock, wake) = &*worker.state;
                    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
                    state.status.running_revision = None;
                    match result {
                        Ok(()) => {
                            state.status.persisted_revision = revision;
                            state.status.failure = None;
                            if state.status.submitted_revision == revision {
                                state.latest = None;
                            }
                        }
                        Err(message) => {
                            state.status.failure = Some(PersistenceFailure { revision, message });
                        }
                    }
                    wake.notify_all();
                }
            });
        if let Err(error) = spawned {
            let mut state = handle.state.0.lock().unwrap_or_else(|e| e.into_inner());
            state.status.stopped = true;
            state.status.failure = Some(PersistenceFailure {
                revision: 0,
                message: format!("Could not start the settings writer: {error}. Restart to retry."),
            });
        }
        Self { handle }
    }

    /// Only a value clone, bounded pointer replacement, and notification occur
    /// on the submitting thread. The single writer preserves commit order.
    pub fn submit(&self, value: I) -> Result<u64, String> {
        let (lock, wake) = &*self.handle.state;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        if state.status.closed {
            return Err("The settings writer is closing.".into());
        }
        let revision = state
            .status
            .submitted_revision
            .checked_add(1)
            .ok_or("The settings revision limit was reached.")?;
        state.status.submitted_revision = revision;
        state.latest = Some(Arc::new(value));
        if state.status.stopped {
            if let Some(failure) = &mut state.status.failure {
                failure.revision = revision;
            }
        } else {
            state.status.pending_revision = Some(revision);
            wake.notify_one();
        }
        Ok(revision)
    }

    /// Failures do not spin or repeatedly hit a full/unavailable disk. An
    /// explicit retry writes the newest retained value, never a stale failure.
    pub fn retry(&self) -> bool {
        let (lock, wake) = &*self.handle.state;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        if state.status.closed || state.status.stopped || !state.status.is_failed() {
            return false;
        }
        state.status.pending_revision = Some(state.status.submitted_revision);
        wake.notify_one();
        true
    }

    pub fn status(&self) -> PersistenceStatus {
        self.handle.status()
    }

    pub fn shutdown_handle(&self) -> PersistenceHandle<I> {
        self.handle.clone()
    }
}

impl<I> PersistenceHandle<I> {
    pub fn status(&self) -> PersistenceStatus {
        self.state
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status
            .clone()
    }

    fn close(&self) {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        state.status.closed = true;
        wake.notify_all();
    }

    /// Call only off the UI thread. Closing rejects later submissions and drains
    /// the last accepted revision. Persistent failure and timeout are explicit.
    pub fn finish(&self, timeout: Duration) -> Result<(), String> {
        self.close();
        let started = Instant::now();
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if state.status.is_saved() {
                return Ok(());
            }
            if state.status.is_failed() {
                return Err(state.status.failure.as_ref().unwrap().message.clone());
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(format!(
                    "Settings revision {} is still unsaved after the shutdown deadline.",
                    state.status.submitted_revision
                ));
            }
            state = wake
                .wait_timeout(state, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl<I> Drop for PersistenceWorker<I> {
    fn drop(&mut self) {
        self.handle.close();
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use std::sync::mpsc;

    fn wait_for<I>(
        handle: &PersistenceHandle<I>,
        predicate: impl Fn(&PersistenceStatus) -> bool,
    ) -> PersistenceStatus {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let status = handle.status();
            if predicate(&status) {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "Writer did not settle: {status:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn blocked_write_keeps_submissions_responsive_and_only_latest_pending() {
        let (started, observed) = mpsc::channel();
        let (release, barrier) = mpsc::channel();
        let worker = PersistenceWorker::new(move |value: &u64| {
            started.send(*value).unwrap();
            if *value == 1 {
                barrier.recv().unwrap();
            }
            Ok(())
        });
        assert_eq!(worker.submit(1).unwrap(), 1);
        assert_eq!(observed.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        let handle = worker.shutdown_handle();
        let (submitted, completed) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            for value in 2..=10_000 {
                assert_eq!(worker.submit(value).unwrap(), value);
            }
            submitted
                .send(worker)
                .unwrap_or_else(|_| panic!("Owner gone"));
        });
        let result = completed.recv_timeout(Duration::from_secs(1));
        // Release even on a regression so a failing test cannot strand its thread.
        let before_release = result.as_ref().ok().map(|_| handle.status());
        release.send(()).unwrap();
        let worker = result.expect("Submitting preferences waited for the blocked writer");
        let before_release = before_release.unwrap();
        assert_eq!(before_release.running_revision, Some(1));
        assert_eq!(before_release.pending_revision, Some(10_000));
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(1)).unwrap(),
            10_000
        );
        assert!(handle.finish(Duration::from_secs(1)).is_ok());
        assert!(observed.recv_timeout(Duration::from_millis(20)).is_err());
        assert_eq!(worker.status().persisted_revision, 10_000);
        producer.join().unwrap();
    }

    #[test]
    fn failed_latest_value_is_retained_for_explicit_retry_without_a_retry_loop() {
        let (attempted, observed) = mpsc::channel();
        let mut attempts = 0;
        let worker = PersistenceWorker::new(move |value: &u64| {
            attempts += 1;
            attempted.send((*value, attempts)).unwrap();
            if attempts == 1 {
                Err("disk full".into())
            } else {
                Ok(())
            }
        });
        worker.submit(42).unwrap();
        let handle = worker.shutdown_handle();
        let failed = wait_for(&handle, PersistenceStatus::is_failed);
        assert_eq!(failed.persisted_revision, 0);
        assert_eq!(failed.failure.as_ref().unwrap().revision, 1);
        assert_eq!(failed.failure.unwrap().message, "disk full");
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(1)).unwrap(),
            (42, 1)
        );
        assert!(observed.recv_timeout(Duration::from_millis(20)).is_err());
        assert!(worker.retry());
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(1)).unwrap(),
            (42, 2)
        );
        let saved = wait_for(&handle, PersistenceStatus::is_saved);
        assert_eq!(saved.submitted_revision, 1);
        assert_eq!(saved.persisted_revision, 1);
        assert!(saved.failure.is_none());
        assert!(!worker.retry());
    }

    #[test]
    fn older_failure_never_replaces_a_newer_pending_value() {
        let (attempted, observed) = mpsc::channel();
        let (release, barrier) = mpsc::channel();
        let worker = PersistenceWorker::new(move |value: &u64| {
            attempted.send(*value).unwrap();
            if *value == 1 {
                barrier.recv().unwrap();
                Err("old save failed".into())
            } else {
                Ok(())
            }
        });
        worker.submit(1).unwrap();
        assert_eq!(observed.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        worker.submit(2).unwrap();
        worker.submit(3).unwrap();
        release.send(()).unwrap();
        assert_eq!(observed.recv_timeout(Duration::from_secs(1)).unwrap(), 3);
        let saved = wait_for(&worker.shutdown_handle(), PersistenceStatus::is_saved);
        assert_eq!(saved.persisted_revision, 3);
        assert!(saved.failure.is_none());
    }

    #[test]
    fn a_new_submission_supersedes_a_retained_failed_value() {
        let (attempted, observed) = mpsc::channel();
        let worker = PersistenceWorker::new(move |value: &u64| {
            attempted.send(*value).unwrap();
            if *value == 1 {
                Err("unavailable".into())
            } else {
                Ok(())
            }
        });
        worker.submit(1).unwrap();
        wait_for(&worker.shutdown_handle(), PersistenceStatus::is_failed);
        worker.submit(2).unwrap();
        assert_eq!(observed.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        assert_eq!(observed.recv_timeout(Duration::from_secs(1)).unwrap(), 2);
        let saved = wait_for(&worker.shutdown_handle(), PersistenceStatus::is_saved);
        assert_eq!(saved.persisted_revision, 2);
        assert!(saved.failure.is_none());
    }

    #[test]
    fn dropping_owner_drains_the_latest_accepted_value() {
        let (attempted, observed) = mpsc::channel();
        let (release, barrier) = mpsc::channel();
        let worker = PersistenceWorker::new(move |value: &u64| {
            attempted.send(*value).unwrap();
            if *value == 1 {
                barrier.recv().unwrap();
            }
            Ok(())
        });
        worker.submit(1).unwrap();
        assert_eq!(observed.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        worker.submit(2).unwrap();
        let handle = worker.shutdown_handle();
        drop(worker);
        assert_eq!(handle.status().pending_revision, Some(2));
        release.send(()).unwrap();
        assert_eq!(observed.recv_timeout(Duration::from_secs(1)).unwrap(), 2);
        assert!(handle.finish(Duration::from_secs(1)).is_ok());
        assert!(wait_for(&handle, |status| status.stopped).is_saved());
    }

    #[test]
    fn shutdown_reports_timeout_and_rejects_submissions_without_losing_accepted_work() {
        let (started, observed) = mpsc::channel();
        let (release, barrier) = mpsc::channel();
        let worker = PersistenceWorker::new(move |_: &u64| {
            started.send(()).unwrap();
            barrier.recv().unwrap();
            Ok(())
        });
        worker.submit(7).unwrap();
        observed.recv_timeout(Duration::from_secs(1)).unwrap();
        let handle = worker.shutdown_handle();
        assert!(
            handle
                .finish(Duration::from_millis(10))
                .unwrap_err()
                .contains("still unsaved")
        );
        assert!(worker.submit(8).is_err());
        release.send(()).unwrap();
        assert!(handle.finish(Duration::from_secs(1)).is_ok());
        assert_eq!(handle.status().persisted_revision, 1);
    }

    #[test]
    fn shutdown_reports_write_failure_instead_of_claiming_the_revision_saved() {
        let worker = PersistenceWorker::new(|_: &u64| Err("permission denied".into()));
        worker.submit(7).unwrap();
        let handle = worker.shutdown_handle();
        assert_eq!(
            handle.finish(Duration::from_secs(1)).unwrap_err(),
            "permission denied"
        );
        assert_eq!(handle.status().persisted_revision, 0);
        assert!(!worker.retry());
    }

    #[test]
    fn writer_panic_becomes_an_observable_failure_and_preserves_retry() {
        let mut first = true;
        let worker = PersistenceWorker::new(move |_: &u64| {
            if first {
                first = false;
                panic!("injected writer failure");
            }
            Ok(())
        });
        worker.submit(7).unwrap();
        let failed = wait_for(&worker.shutdown_handle(), PersistenceStatus::is_failed);
        assert!(failed.failure.unwrap().message.contains("unexpectedly"));
        assert!(worker.retry());
        assert!(wait_for(&worker.shutdown_handle(), PersistenceStatus::is_saved).is_saved());
    }

    /// No GUI, native linking, user settings, or real disk writes are involved.
    /// Run with: cargo test --lib benchmark_preference_submissions -- --ignored --nocapture
    #[test]
    #[ignore = "manual non-GUI submission latency benchmark"]
    fn benchmark_preference_submissions() {
        let (started, observed) = mpsc::channel();
        let (release, barrier) = mpsc::channel();
        let worker = PersistenceWorker::new(move |value: &(usize, Vec<u8>)| {
            started.send(value.0).unwrap();
            if value.0 == 0 {
                barrier.recv().unwrap();
            }
            Ok(())
        });
        const SUBMISSIONS: usize = 10_000;
        let payload = vec![b'x'; 4096];
        worker.submit((0, payload.clone())).unwrap();
        assert_eq!(observed.recv_timeout(Duration::from_secs(1)).unwrap(), 0);
        let mut timings = Vec::with_capacity(SUBMISSIONS);
        let started = Instant::now();
        for revision in 1..=SUBMISSIONS {
            let at = Instant::now();
            worker.submit((revision, payload.clone())).unwrap();
            timings.push(at.elapsed().as_nanos());
        }
        let elapsed = started.elapsed();
        release.send(()).unwrap();
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(1)).unwrap(),
            SUBMISSIONS
        );
        assert!(
            worker
                .shutdown_handle()
                .finish(Duration::from_secs(1))
                .is_ok()
        );
        assert!(observed.recv_timeout(Duration::from_millis(20)).is_err());
        timings.sort_unstable();
        println!(
            "preference_submit_benchmark submissions={SUBMISSIONS} payload_bytes={} blocked_writer=true writes=2 total_us={} p50_ns={} p95_ns={} max_ns={}",
            payload.len(),
            elapsed.as_micros(),
            timings[SUBMISSIONS / 2],
            timings[SUBMISSIONS * 95 / 100],
            timings[SUBMISSIONS - 1],
        );
    }
}

/// One running operation and one replaceable pending request. A slow disk or
/// subprocess never creates an unbounded queue of refreshes.
pub struct LatestWorker<I> {
    state: Arc<(Mutex<State<I>>, Condvar)>,
}
struct State<I> {
    pending: Option<I>,
    closed: bool,
}
impl<I: Send + 'static> LatestWorker<I> {
    pub fn new(mut operation: impl FnMut(I) + Send + 'static) -> Self {
        let state = Arc::new((
            Mutex::new(State {
                pending: None,
                closed: false,
            }),
            Condvar::new(),
        ));
        let worker = state.clone();
        std::thread::spawn(move || {
            loop {
                let request = {
                    let (lock, wake) = &*worker;
                    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while state.pending.is_none() && !state.closed {
                        state = wake.wait(state).unwrap_or_else(|e| e.into_inner());
                    }
                    if state.closed {
                        break;
                    }
                    state.pending.take().unwrap()
                };
                operation(request);
            }
        });
        Self { state }
    }
    pub fn request(&self, input: I) {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        state.pending = Some(input);
        wake.notify_one();
    }
    /// Background polling may fill an empty slot, but cannot replace a queued
    /// explicit/catalog request while an earlier operation is still running.
    pub fn request_if_no_pending(&self, input: I) -> bool {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed || state.pending.is_some() {
            return false;
        }
        state.pending = Some(input);
        wake.notify_one();
        true
    }
}
impl<I> Drop for LatestWorker<I> {
    fn drop(&mut self) {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        state.pending = None;
        wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};
    #[test]
    fn refreshes_keep_only_the_latest_pending_request() {
        let (tx, rx) = mpsc::channel();
        let (release, barrier) = mpsc::channel();
        let worker = LatestWorker::new(move |value| {
            tx.send(value).unwrap();
            if value == 1 {
                barrier.recv().unwrap();
            }
        });
        worker.request(1);
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        worker.request(2);
        worker.request(3);
        worker.request(4);
        assert!(!worker.request_if_no_pending(5));
        release.send(()).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), 4);
        assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());
    }
}

pub fn retained_record_index(records: &[Record], selected_id: Option<&str>) -> Option<usize> {
    let id = selected_id?;
    records.iter().position(|record| record.id == id)
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    #[test]
    fn record_selection_survives_tail_window_eviction_by_identity() {
        let records = vec![
            Record {
                id: "b".into(),
                ..Default::default()
            },
            Record {
                id: "c".into(),
                ..Default::default()
            },
        ];
        assert_eq!(retained_record_index(&records, Some("b")), Some(0));
        assert_eq!(retained_record_index(&records, Some("a")), None);
        assert_eq!(retained_record_index(&records, None), None);
    }
}
