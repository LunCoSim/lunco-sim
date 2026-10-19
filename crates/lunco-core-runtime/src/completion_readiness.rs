//! Native host readiness without a second consumer of participant receipts.

use bevy::prelude::Resource;
use crossbeam_channel::{Receiver, Select};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
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
    sources: Vec<Box<dyn CompletionSource>>,
}

impl SimulationCompletionSources {
    /// Register one worker's result channel once during host assembly.
    pub fn register<T: Send + 'static>(
        &mut self,
        receiver: &Receiver<T>,
    ) -> SimulationCompletionLease {
        let alive = Arc::new(AtomicBool::new(true));
        self.sources.push(Box::new(ChannelSource {
            receiver: receiver.clone(),
            alive: alive.clone(),
        }));
        SimulationCompletionLease(alive)
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
