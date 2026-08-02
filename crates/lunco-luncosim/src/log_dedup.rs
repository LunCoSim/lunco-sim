//! Collapse repeated WARN/ERROR log lines into a single line plus a count.
//!
//! The report's log-noise bulk is a handful of *identical* warnings fired every
//! frame (camera 40×, port 74×, font 16× in one startup). Raising the level or
//! silencing the site loses the signal; what we want is "show it once, then tell
//! me it kept happening N more times" — the classic syslog `last message
//! repeated N times`.
//!
//! ## Why a per-layer [`Filter`], not a formatter or writer
//!
//! Dedup must key on the *message*, and the message is only cheaply available at
//! the event level — a formatted line also carries a per-event timestamp, so two
//! "identical" warnings never compare equal once formatted. A
//! [`tracing_subscriber`] per-layer [`Filter`] sees the raw event: its
//! `event_enabled` returning `false` drops that event for the fmt layer only,
//! which is exactly "suppress this line."
//!
//! ## Why counts are flushed by a Bevy system, not emitted inline
//!
//! A filter cannot emit — and must not: `event_enabled` runs while holding the
//! shared state lock, so logging from inside it would re-enter the filter and
//! deadlock. Instead the filter just *counts* suppressed repeats, and
//! [`flush_dedup_summaries`] (an ordinary `Update` system, no lock held while it
//! logs) drains those counts into one summary line per burst. The summary is
//! logged under [`SUMMARY_TARGET`], which the filter always passes, so it can
//! never dedup its own output.
//!
//! Only WARN and ERROR are deduped; INFO/DEBUG/TRACE pass untouched, so the hot
//! logging path pays nothing but a level comparison.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bevy::log::tracing::field::{Field, Visit};
use bevy::log::tracing::{Event, Level, Metadata};
use bevy::log::tracing_subscriber::layer::{Context, Filter};
use bevy::prelude::*;

/// How long a distinct WARN/ERROR line stays "recently shown". A repeat inside
/// this window is suppressed and counted; once it elapses the next occurrence is
/// shown again (fresh), so a genuinely recurring condition still resurfaces.
const WINDOW: Duration = Duration::from_secs(3);

/// Drop map entries untouched for this long, so a one-shot warning during
/// startup does not pin memory for the whole session.
const PRUNE_AFTER: Duration = Duration::from_secs(60);

/// Target for the synthetic summary lines. The filter always passes this target,
/// so its own output is never a dedup candidate (which would re-enter the lock).
const SUMMARY_TARGET: &str = "lunco::log_dedup";

struct Entry {
    /// The message text as first seen, replayed in the summary line.
    message: String,
    /// When this line was last actually shown (not suppressed).
    last_shown: Instant,
    /// Repeats swallowed since `last_shown`, awaiting a summary flush.
    suppressed: u64,
}

#[derive(Default)]
struct DedupState {
    seen: HashMap<u64, Entry>,
}

/// Global, because the tracing subscriber is global and is built (in
/// `LogPlugin`) before the `App` exists. The filter and the flush system share
/// this one handle.
static STATE: OnceLock<Arc<Mutex<DedupState>>> = OnceLock::new();

fn state() -> &'static Arc<Mutex<DedupState>> {
    STATE.get_or_init(|| Arc::new(Mutex::new(DedupState::default())))
}

/// Pulls the `message` field out of an event so identical warnings hash equal.
#[derive(Default)]
struct MsgVisitor {
    msg: Option<String>,
}

impl Visit for MsgVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.msg = Some(format!("{value:?}"));
        }
    }
}

/// Per-layer filter that suppresses repeated WARN/ERROR lines within [`WINDOW`].
pub(crate) struct DedupFilter;

impl<S> Filter<S> for DedupFilter {
    fn enabled(&self, _meta: &Metadata<'_>, _cx: &Context<'_, S>) -> bool {
        // Interest is level-independent here; the real work is per-event.
        true
    }

    fn event_enabled(&self, event: &Event<'_>, _cx: &Context<'_, S>) -> bool {
        let meta = event.metadata();
        // tracing orders levels by verbosity (TRACE > … > WARN > ERROR), so
        // `> WARN` is exactly INFO/DEBUG/TRACE — pass those untouched. Also pass
        // our own summary lines so we never dedup them.
        if *meta.level() > Level::WARN || meta.target() == SUMMARY_TARGET {
            return true;
        }

        let mut visitor = MsgVisitor::default();
        event.record(&mut visitor);
        let msg = visitor.msg.unwrap_or_default();

        let mut hasher = DefaultHasher::new();
        meta.target().hash(&mut hasher);
        meta.level().as_str().hash(&mut hasher);
        msg.hash(&mut hasher);
        let key = hasher.finish();

        let now = Instant::now();
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        match st.seen.get_mut(&key) {
            Some(e) if now.duration_since(e.last_shown) < WINDOW => {
                e.suppressed += 1;
                false
            }
            Some(e) => {
                e.last_shown = now;
                true
            }
            None => {
                st.seen.insert(
                    key,
                    Entry {
                        message: msg,
                        last_shown: now,
                        suppressed: 0,
                    },
                );
                true
            }
        }
    }
}

/// Registers the summary-flush system. The filter itself is installed on the
/// fmt layer in `LogPlugin` (see `crate::default_plugins`).
pub(crate) struct LogDedupPlugin;

impl Plugin for LogDedupPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, flush_dedup_summaries);
    }
}

/// Emits one summary line per line that swallowed repeats, then clears the
/// counters. Runs on real time so it still fires while the sim clock is paused.
fn flush_dedup_summaries(time: Res<Time<Real>>, mut acc: Local<f32>) {
    *acc += time.delta_secs();
    if *acc < WINDOW.as_secs_f32() {
        return;
    }
    *acc = 0.0;

    let now = Instant::now();
    // Collect under the lock, then log after releasing it — logging re-enters the
    // filter, which takes the same lock.
    let summaries: Vec<(String, u64)> = {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        st.seen
            .retain(|_, e| now.duration_since(e.last_shown) < PRUNE_AFTER);
        st.seen
            .values_mut()
            .filter(|e| e.suppressed > 0)
            .map(|e| {
                let out = (e.message.clone(), e.suppressed);
                e.suppressed = 0;
                out
            })
            .collect()
    };

    for (message, count) in summaries {
        warn!(target: SUMMARY_TARGET, "{message} (+{count} identical suppressed)");
    }
}
