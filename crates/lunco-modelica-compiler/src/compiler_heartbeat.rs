//! Native compiler-owned progress lifetime. Shutdown interrupts the diagnostic
//! interval and joins the thread before normal return, error return, or unwind.

use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::thread::{Builder, JoinHandle};
use std::time::Duration;

pub(super) struct CompileHeartbeat {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl CompileHeartbeat {
    pub(super) fn start(model_name: &str) -> std::io::Result<Self> {
        let model_name = model_name.to_owned();
        let started = web_time::Instant::now();
        Self::spawn(Duration::from_secs(5), move || {
            log::info!(
                "[ModelicaCompiler] still compiling `{}` (+{:.0}s)",
                model_name,
                started.elapsed().as_secs_f64(),
            );
        })
    }

    fn spawn(
        interval: Duration,
        mut on_tick: impl FnMut() + Send + 'static,
    ) -> std::io::Result<Self> {
        let (stop, stopped) = channel();
        let thread = Builder::new()
            .name("modelica-compile-heartbeat".into())
            .spawn(move || {
                loop {
                    match stopped.recv_timeout(interval) {
                        Err(RecvTimeoutError::Timeout) => on_tick(),
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })?;
        Ok(Self {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

impl Drop for CompileHeartbeat {
    fn drop(&mut self) {
        // Disconnect wakes recv_timeout immediately, including during unwind.
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                log::warn!("[ModelicaCompiler] compile heartbeat thread panicked");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Retired(Arc<AtomicBool>);
    impl Drop for Retired {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn tracked(retired: Arc<AtomicBool>) -> CompileHeartbeat {
        let capture = Retired(retired);
        CompileHeartbeat::spawn(Duration::from_secs(60), move || {
            let _ = &capture;
        })
        .unwrap()
    }

    #[test]
    fn compile_heartbeat_retires_and_interrupts_wait_on_normal_error_and_unwind() {
        let started = std::time::Instant::now();
        let normal_retired = Arc::new(AtomicBool::new(false));
        {
            let _heartbeat = tracked(Arc::clone(&normal_retired));
        }
        assert!(normal_retired.load(Ordering::SeqCst));

        let error_retired = Arc::new(AtomicBool::new(false));
        let result: Result<(), &str> = (|| {
            let _heartbeat = tracked(Arc::clone(&error_retired));
            Err("compiler rejected source")
        })();
        assert!(result.is_err());
        assert!(error_retired.load(Ordering::SeqCst));

        let panic_retired = Arc::new(AtomicBool::new(false));
        let outcome = std::panic::catch_unwind(|| {
            let _heartbeat = tracked(Arc::clone(&panic_retired));
            panic!("compiler boundary unwind");
        });
        assert!(outcome.is_err());
        assert!(panic_retired.load(Ordering::SeqCst));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn compile_heartbeat_emits_ticks_only_while_its_owner_is_live() {
        let (tick, ticks) = channel();
        let heartbeat = CompileHeartbeat::spawn(Duration::from_millis(1), move || {
            let _ = tick.send(());
        })
        .unwrap();
        ticks.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(heartbeat);
        while ticks.try_recv().is_ok() {}
        assert_eq!(
            ticks.recv_timeout(Duration::from_secs(1)),
            Err(RecvTimeoutError::Disconnected)
        );
    }
}
