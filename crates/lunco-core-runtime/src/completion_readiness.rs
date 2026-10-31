//! Native host readiness without a second consumer of participant receipts.

use bevy::prelude::Resource;
use crossbeam_channel::{Receiver, Select, Sender};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::task::{Context, Poll, Waker};
use std::time::Instant;

trait CompletionSource: Send + Sync {
    fn register<'a>(&'a self, select: &mut Select<'a>);
    fn has_samples(&self) -> bool;
    fn is_alive(&self) -> bool;
}

struct ChannelSource<T> {
    receiver: Receiver<T>,
    alive: Arc<AtomicBool>,
}
impl<T: Send> CompletionSource for ChannelSource<T> {
    fn register<'a>(&'a self, select: &mut Select<'a>) {
        select.recv(&self.receiver);
    }
    fn has_samples(&self) -> bool {
        !self.receiver.is_empty()
    }
    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
}

/// Retain on the worker's stack for its complete native lifetime. This only
/// retires readiness registration; the domain owns failure and receipt handling.
#[must_use = "retain the completion lease for the worker's entire lifetime"]
pub struct SimulationCompletionLease(Arc<AtomicBool>);
impl Drop for SimulationCompletionLease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Readiness registrations for native simulation workers. Only their owning
/// `SimulationCompletions` systems receive samples. This host service observes
/// channel readiness and never chooses authoritative result application order.
#[derive(Resource, Default)]
pub struct SimulationCompletionSources {
    sources: Vec<Arc<dyn CompletionSource>>,
    wake: Option<CompletionWake>,
}

/// One native waiter observes readiness without receiving domain results. It
/// coalesces notifications until the host services the admission schedule.
struct CompletionWake {
    sources: Arc<Mutex<Vec<Arc<dyn CompletionSource>>>>,
    rearm: Sender<()>,
    armed: Arc<AtomicBool>,
    waiter: Arc<Mutex<Option<Waker>>>,
}

impl CompletionWake {
    fn notify(&self) {
        // A full channel already requests a refresh. Disconnection means the
        // host's event loop and its readiness waiter have been retired.
        let _ = self.rearm.try_send(());
    }
}

impl SimulationCompletionSources {
    /// Register one worker's result channel once during host assembly.
    pub fn register<T: Send + 'static>(
        &mut self,
        receiver: &Receiver<T>,
    ) -> SimulationCompletionLease {
        let alive = Arc::new(AtomicBool::new(true));
        self.sources.push(Arc::new(ChannelSource {
            receiver: receiver.clone(),
            alive: alive.clone(),
        }));
        if let Some(wake) = &self.wake {
            *wake
                .sources
                .lock()
                .expect("completion source registry poisoned") = self.sources.clone();
            wake.notify();
        }
        SimulationCompletionLease(alive)
    }

    /// Install the native window host's event-loop notification. The callback
    /// only wakes the host; the existing owner still admits every receipt.
    /// No UI or simulation thread waits for a worker result.
    pub fn install_waker(&mut self, wake_host: Arc<dyn Fn() + Send + Sync>) -> std::io::Result<()> {
        if self.wake.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "simulation completion waker is already installed",
            ));
        }
        let sources = Arc::new(Mutex::new(self.sources.clone()));
        let watched = sources.clone();
        let armed = Arc::new(AtomicBool::new(true));
        let waiter_armed = armed.clone();
        let waiter = Arc::new(Mutex::new(None::<Waker>));
        let waiter_notification = waiter.clone();
        let (rearm, notices) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("simulation-ready".into())
            .spawn(move || {
                loop {
                    if !waiter_armed.load(Ordering::Acquire) {
                        if notices.recv().is_err() {
                            return;
                        }
                        continue;
                    }
                    let snapshot: Vec<_> = watched
                        .lock()
                        .expect("completion source registry poisoned")
                        .iter()
                        .filter(|source| source.is_alive() || source.has_samples())
                        .cloned()
                        .collect();
                    let mut select = Select::new();
                    let refresh = select.recv(&notices);
                    for source in &snapshot {
                        source.register(&mut select);
                    }
                    if select.ready() == refresh {
                        if notices.recv().is_err() {
                            return;
                        }
                        continue;
                    }
                    // Ready may be a closed worker channel as well as a receipt.
                    // Let its owner diagnose that through the same admission path.
                    if waiter_armed.swap(false, Ordering::AcqRel) {
                        wake_host();
                        if let Some(waker) = waiter_notification
                            .lock()
                            .expect("completion waiter poisoned")
                            .take()
                        {
                            waker.wake();
                        }
                    }
                }
            })?;
        self.wake = Some(CompletionWake {
            sources,
            rearm,
            armed,
            waiter,
        });
        Ok(())
    }

    /// Register the native host's current render-wait task for the next receipt.
    /// This observes the same coalesced notification as the window callback.
    pub fn poll_wake(&self, cx: &mut Context<'_>) -> Result<Poll<()>, &'static str> {
        let wake = self
            .wake
            .as_ref()
            .ok_or("simulation completion waker is not installed")?;
        *wake.waiter.lock().expect("completion waiter poisoned") = Some(cx.waker().clone());
        Ok(if wake.armed.load(Ordering::Acquire) {
            Poll::Pending
        } else {
            Poll::Ready(())
        })
    }

    /// Rearm readiness notification after the sole receipt consumer has run.
    pub(crate) fn rearm_waker(&mut self) {
        let count = self.sources.len();
        self.sources
            .retain(|source| source.is_alive() || source.has_samples());
        if let Some(wake) = &self.wake {
            if self.sources.len() != count {
                *wake
                    .sources
                    .lock()
                    .expect("completion source registry poisoned") = self.sources.clone();
            }
            wake.armed.store(true, Ordering::Release);
            wake.notify();
        }
    }

    /// Sleep until any worker has a receipt or the native pacing deadline.
    /// Window/UI hosts must never call this blocking native-host operation.
    pub fn wait_until_ready_or(&mut self, deadline: Instant) {
        loop {
            self.sources
                .retain(|source| source.is_alive() || source.has_samples());
            if self.sources.iter().any(|source| source.has_samples()) || Instant::now() >= deadline
            {
                return;
            }
            if self.sources.is_empty() {
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                return;
            }
            {
                let mut ready = Select::new();
                for source in &self.sources {
                    source.register(&mut ready);
                }
                if ready.ready_deadline(deadline).is_err() {
                    return;
                }
            }
            // Crossbeam allows spurious readiness. Only the worker lease can
            // retire a source; an empty sample queue never proves termination.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_wakes_host_once_until_admission_and_preserves_receipts() {
        let mut sources = SimulationCompletionSources::default();
        let mut context = Context::from_waker(Waker::noop());
        assert!(sources.poll_wake(&mut context).is_err());
        let (woken, wakes) = crossbeam_channel::unbounded();
        sources
            .install_waker(Arc::new(move || {
                let _ = woken.send(());
            }))
            .unwrap();
        assert_eq!(sources.poll_wake(&mut context), Ok(Poll::Pending));
        let (tx, rx) = crossbeam_channel::unbounded::<u64>();
        let lease = sources.register(&rx);
        tx.send(7).unwrap();
        wakes
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(sources.poll_wake(&mut context), Ok(Poll::Ready(())));
        assert_eq!(rx.try_recv(), Ok(7), "wake-up must not consume the receipt");
        tx.send(8).unwrap();
        assert!(
            wakes.try_recv().is_err(),
            "an outstanding wake is coalesced"
        );
        sources.rearm_waker();
        wakes
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(rx.try_recv(), Ok(8));
        drop(tx);
        drop(lease);
        sources.rearm_waker();
        // A newly registered source must join the same waiter after retirement.
        let (tx, rx) = crossbeam_channel::unbounded::<u64>();
        let _lease = sources.register(&rx);
        tx.send(9).unwrap();
        wakes
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(rx.try_recv(), Ok(9));
        drop(sources);
        assert!(
            matches!(
                wakes.recv_timeout(std::time::Duration::from_secs(2)),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected)
            ),
            "retiring the host must release its readiness waiter"
        );
    }

    #[test]
    fn mixed_worker_readiness_preserves_samples_and_retires_disconnected_sources() {
        let (a_tx, a_rx) = crossbeam_channel::unbounded::<u64>();
        let (b_tx, b_rx) = crossbeam_channel::unbounded::<String>();
        let mut sources = SimulationCompletionSources::default();
        let a_lease = sources.register(&a_rx);
        let b_lease = sources.register(&b_rx);
        a_tx.send(7).unwrap();
        b_tx.send("ready".into()).unwrap();
        sources.wait_until_ready_or(Instant::now());
        assert_eq!(a_rx.try_recv(), Ok(7));
        sources.wait_until_ready_or(Instant::now());
        assert_eq!(b_rx.try_recv(), Ok("ready".into()));
        drop(a_tx);
        sources.wait_until_ready_or(Instant::now());
        assert_eq!(
            sources.sources.len(),
            2,
            "an empty ready queue does not retire a live worker"
        );
        drop(a_lease);
        sources.wait_until_ready_or(Instant::now());
        assert_eq!(sources.sources.len(), 1);
        drop(b_tx);
        drop(b_lease);
        sources.wait_until_ready_or(Instant::now());
        assert!(sources.sources.is_empty());
    }
}
