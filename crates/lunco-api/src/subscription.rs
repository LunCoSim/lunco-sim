//! Telemetry subscription system — streams telemetry events to API subscribers.

use crate::{
    executor::ApiResponseEvent,
    schema::{ApiResponse, TelemetryFilter, TelemetryResponse},
};
use bevy::prelude::*;
use lunco_core::SessionId;

/// Telemetry events ride the same `ApiResponseEvent` channel as HTTP
/// request/response, but they are server-pushed packets, not replies to a
/// request. The HTTP router mints request correlation ids from a `Local<u64>`
/// counting up from 1; if telemetry reused that space, a telemetry packet
/// whose id happened to match a pending request would resolve (and steal)
/// that request's oneshot in `http_response_observer`. Setting the top bit
/// carves out a disjoint id space so the two can never collide. CQ-509.
const TELEMETRY_CORRELATION_FLAG: u64 = 1 << 63;

/// Active telemetry subscription.
///
/// # Who owns a subscription, and who can reap it
///
/// Every subscription is attributed to the [`SessionId`] that asked for it, so
/// "drop everything this session held" is expressible ([`
/// TelemetrySubscriptions::release_session`]) instead of impossible. Before this
/// field existed a subscription was an anonymous row: `unsubscribe(id)` was the
/// ONLY way to remove one, so a subscriber that went away without calling it
/// leaked its row for the life of the process.
///
/// **The single-player leak is closed by construction, not by a reaper.** Local
/// callers subscribe as [`SessionId::LOCAL`], whose lifetime IS the process
/// lifetime — there is no moment at which a `LOCAL` subscription is garbage, so
/// nothing needs to collect it, and the explicit `UnsubscribeTelemetry` path
/// already covers a client that leaves politely.
///
/// **The remote leak is still open**, and honestly so. Nothing in the API layer
/// observes a peer disconnecting; the only site that learns of one is
/// `lunco-networking/src/server.rs::on_server_disconnected`, which must call
/// [`TelemetrySubscriptions::release_session`] with the departing session. Until
/// it does, a remote client that drops without unsubscribing still leaks — the
/// field and the reaper exist so that wiring is a one-line change, not a
/// refactor. See the matching TODO in that observer.
/// (report_glm52.md CONC-1 / Tier B7.)
///
/// The ingress side is the other half of that work: `ApiRequestEvent` carries no
/// session, so [`TelemetrySubscriptions::subscribe`] can only stamp `LOCAL`. A
/// transport that knows its peer calls [`TelemetrySubscriptions::subscribe_as`].
#[derive(Debug)]
pub struct TelemetrySubscription {
    pub id: u64,
    pub filter: TelemetryFilter,
    /// Session that created this subscription — the reaping key.
    /// [`SessionId::LOCAL`] for anything the local process asked for.
    pub owner: SessionId,
}

impl TelemetrySubscription {
    /// An empty name list means "everything".
    fn matches_name(&self, name: &str) -> bool {
        self.filter.names.is_empty() || self.filter.names.iter().any(|n| n == name)
    }
}

/// Registry of active telemetry subscriptions.
#[derive(Resource, Default)]
pub struct TelemetrySubscriptions {
    subscriptions: Vec<TelemetrySubscription>,
    next_id: u64,
    next_correlation_id: u64,
    /// Last delivered sim-time per `(channel name, source entity bits)`, for
    /// `TelemetryFilter::rate_hz` decimation. Keyed by entity as well as name because
    /// names are not unique — two rovers' `"motor_current"` must not throttle each other.
    last_sent: std::collections::HashMap<(String, u64), f64>,
}

impl TelemetrySubscriptions {
    /// Subscribe on behalf of the LOCAL session.
    ///
    /// This is what the executor calls: `ApiRequestEvent` carries no session, so
    /// the API layer cannot honestly claim a request came from anyone else. A
    /// transport that DOES know its peer must call [`Self::subscribe_as`] —
    /// otherwise the row is attributed to `LOCAL` and
    /// [`Self::release_session`] can never reap it.
    pub fn subscribe(&mut self, filter: Option<TelemetryFilter>) -> u64 {
        self.subscribe_as(SessionId::LOCAL, filter)
    }

    /// Subscribe on behalf of `owner`, so the row can be reaped with that
    /// session by [`Self::release_session`].
    pub fn subscribe_as(&mut self, owner: SessionId, filter: Option<TelemetryFilter>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.subscriptions.push(TelemetrySubscription {
            id,
            filter: filter.unwrap_or_default(),
            owner,
        });
        id
    }
    pub fn unsubscribe(&mut self, id: u64) {
        self.subscriptions.retain(|s| s.id != id);
        self.forget_watermarks_if_idle();
    }

    /// Drop every subscription owned by `session` and report how many went.
    ///
    /// The reaper the leak needs. It is deliberately a plain method rather than
    /// an observer: the API layer never learns that a session ended, so wiring
    /// it to a disconnect signal here would be a reaper that never runs. The one
    /// site that does learn is
    /// `lunco-networking/src/server.rs::on_server_disconnected` — it calls this.
    ///
    /// Refuses `SessionId::LOCAL`: the local session outlives the registry, so
    /// "release LOCAL" is never a real disconnect, and honouring it would let a
    /// stray call silently mute all local telemetry.
    pub fn release_session(&mut self, session: SessionId) -> usize {
        if session == SessionId::LOCAL {
            return 0;
        }
        let before = self.subscriptions.len();
        self.subscriptions.retain(|s| s.owner != session);
        let dropped = before - self.subscriptions.len();
        self.forget_watermarks_if_idle();
        dropped
    }

    /// Drop the decimation bookkeeping when the last subscriber leaves, so a
    /// later subscriber isn't silently throttled by a dead one's watermark.
    fn forget_watermarks_if_idle(&mut self) {
        if self.subscriptions.is_empty() {
            self.last_sent.clear();
        }
    }

    /// Rate-limit a SAMPLED parameter against the matching subscriptions'
    /// `TelemetryFilter::rate_hz`.
    ///
    /// Returns `true` if the sample should go out now. Because telemetry is one shared
    /// stream rather than a per-subscriber fan-out (see `TelemetryFilter::rate_hz`), the
    /// gate is the FASTEST rate any matching subscriber asked for — throttling to the
    /// slowest would starve a client that explicitly asked for full rate.
    ///
    /// `sim_secs` (not the Julian-Date `timestamp`) is the clock here: JD has ~86 µs of
    /// f64 resolution left, so differencing two of them to test "has 1/rate elapsed"
    /// would be noise.
    fn should_send_sample(&mut self, name: &str, source_bits: u64, sim_secs: f64) -> bool {
        if !self.should_broadcast(name, None) {
            return false;
        }

        // Fastest requested rate among the subscriptions that actually match this name.
        // Any matching subscriber with no rate cap means "send everything".
        let mut fastest: Option<f64> = None;
        for sub in self.subscriptions.iter().filter(|s| s.matches_name(name)) {
            match sub.filter.rate_hz {
                None => return self.mark_sent(name, source_bits, sim_secs),
                Some(r) if r.is_finite() && r > 0.0 => {
                    fastest = Some(fastest.map_or(r, |f: f64| f.max(r)));
                }
                // A nonsense cap (0, negative, NaN) is treated as "no cap" rather than
                // silently muting the channel forever.
                Some(_) => return self.mark_sent(name, source_bits, sim_secs),
            }
        }

        let Some(rate) = fastest else {
            return self.mark_sent(name, source_bits, sim_secs);
        };

        let key = (name.to_string(), source_bits);
        match self.last_sent.get(&key) {
            Some(&last) if sim_secs - last < 1.0 / rate => false,
            _ => self.mark_sent(name, source_bits, sim_secs),
        }
    }

    fn mark_sent(&mut self, name: &str, source_bits: u64, sim_secs: f64) -> bool {
        self.last_sent
            .insert((name.to_string(), source_bits), sim_secs);
        true
    }
    fn should_broadcast(&self, name: &str, severity: Option<lunco_core::Severity>) -> bool {
        if self.subscriptions.is_empty() {
            return false;
        }
        self.subscriptions.iter().any(|sub| {
            let name_ok =
                sub.filter.names.is_empty() || sub.filter.names.contains(&name.to_string());
            let severity_ok = match (severity, &sub.filter.min_severity) {
                (None, _) => true,
                (Some(_), None) => true,
                (Some(sev), Some(min_str)) => {
                    let min = match min_str.as_str() {
                        "Debug" => lunco_core::Severity::Debug,
                        "Info" => lunco_core::Severity::Info,
                        "Warning" => lunco_core::Severity::Warning,
                        "Error" => lunco_core::Severity::Error,
                        "Critical" => lunco_core::Severity::Critical,
                        _ => lunco_core::Severity::Debug,
                    };
                    sev >= min
                }
            };
            name_ok && severity_ok
        })
    }
    fn next_correlation_id(&mut self) -> u64 {
        let id = self.next_correlation_id;
        self.next_correlation_id += 1;
        id | TELEMETRY_CORRELATION_FLAG
    }
}

/// Observer for sampled parameters.
pub fn sampled_param_observer(
    trigger: On<lunco_core::telemetry::SampledParameter>,
    mut subscriptions: ResMut<TelemetrySubscriptions>,
    // Parameter names are not unique across entities, so a subscriber needs the
    // owning entity's global id to tell two `"motor_current"`s apart.
    ids: Query<&lunco_core::GlobalEntityId>,
    mut commands: Commands,
) {
    let sample = trigger.event();
    // Name/severity filter AND the per-subscription rate cap.
    if !subscriptions.should_send_sample(&sample.name, sample.source.to_bits(), sample.sim_secs) {
        return;
    }
    let source = ids.get(sample.source).ok().map(|g| g.get());
    let correlation_id = subscriptions.next_correlation_id();
    commands.trigger(ApiResponseEvent {
        correlation_id,
        response: ApiResponse::TelemetryEvent(TelemetryResponse::from_sampled(sample, source)),
    });
}

/// Observer for telemetry events.
pub fn telemetry_event_observer(
    trigger: On<lunco_core::telemetry::TelemetryEvent>,
    mut subscriptions: ResMut<TelemetrySubscriptions>,
    mut commands: Commands,
) {
    let event = trigger.event();
    if !subscriptions.should_broadcast(&event.name, Some(event.severity)) {
        return;
    }
    let correlation_id = subscriptions.next_correlation_id();
    commands.trigger(ApiResponseEvent {
        correlation_id,
        response: ApiResponse::TelemetryEvent(TelemetryResponse::from_event(event)),
    });
}

/// Plugin that registers telemetry subscription observers.
pub struct ApiTelemetryPlugin;
impl Plugin for ApiTelemetryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TelemetrySubscriptions>()
            .add_observer(sampled_param_observer)
            .add_observer(telemetry_event_observer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscribe_unsubscribe() {
        let mut subs = TelemetrySubscriptions::default();
        let id = subs.subscribe(None);
        subs.unsubscribe(id);
        assert!(subs.subscriptions.is_empty());
    }

    /// A subscription is attributed to its session, and releasing that session
    /// takes exactly its rows — a second session's stream must survive the
    /// first one's disconnect.
    #[test]
    fn releasing_a_session_reaps_only_its_own_subscriptions() {
        let mut subs = TelemetrySubscriptions::default();
        let a = SessionId(1);
        let b = SessionId(2);
        subs.subscribe_as(a, None);
        subs.subscribe_as(a, None);
        subs.subscribe_as(b, None);

        assert_eq!(subs.release_session(a), 2);
        assert_eq!(subs.subscriptions.len(), 1);
        assert_eq!(subs.subscriptions[0].owner, b);
        assert!(
            subs.should_broadcast("any_name", None),
            "the surviving session must still receive telemetry"
        );
    }

    /// `LOCAL` is the process itself: it never disconnects, so releasing it is
    /// always a mistake and must not mute local telemetry.
    #[test]
    fn releasing_the_local_session_is_refused() {
        let mut subs = TelemetrySubscriptions::default();
        subs.subscribe(None); // stamped LOCAL — the executor's path
        assert_eq!(subs.release_session(SessionId::LOCAL), 0);
        assert_eq!(subs.subscriptions.len(), 1);
        assert_eq!(subs.subscriptions[0].owner, SessionId::LOCAL);
    }

    #[test]
    fn test_broadcast_with_default_filter() {
        let mut subs = TelemetrySubscriptions::default();
        subs.subscribe(None);
        assert!(subs.should_broadcast("any_name", None));
        assert!(subs.should_broadcast("any_name", Some(lunco_core::Severity::Critical)));
    }

    #[test]
    fn test_severity_filter() {
        let mut subs = TelemetrySubscriptions::default();
        subs.subscribe(Some(TelemetryFilter {
            names: vec![],
            min_severity: Some("Warning".to_string()),
            rate_hz: None,
        }));
        assert!(!subs.should_broadcast("alert", Some(lunco_core::Severity::Debug)));
        assert!(subs.should_broadcast("alert", Some(lunco_core::Severity::Warning)));
        assert!(subs.should_broadcast("alert", Some(lunco_core::Severity::Critical)));
    }

    #[test]
    fn test_telemetry_correlation_ids_are_disjoint_from_http() {
        let mut subs = TelemetrySubscriptions::default();
        // HTTP correlation ids count up from 1 in the low (u32) range; a
        // telemetry id landing there would steal a pending request's reply.
        for _ in 0..1000 {
            let cid = subs.next_correlation_id();
            assert!(
                cid & TELEMETRY_CORRELATION_FLAG != 0,
                "telemetry id {cid} lacks disjoint flag"
            );
            assert!(
                cid > u32::MAX as u64,
                "telemetry id {cid} collides with HTTP low-id range"
            );
        }
    }

    /// A subscription rate cap decimates the SHARED stream. `sim_secs` is the clock —
    /// not the Julian-Date timestamp, whose f64 resolution at JD magnitudes (~86 µs) makes
    /// "has 1/rate elapsed?" meaningless.
    #[test]
    fn a_subscription_rate_cap_decimates_the_stream() {
        let mut subs = TelemetrySubscriptions::default();
        subs.subscribe(Some(TelemetryFilter {
            names: vec![],
            min_severity: None,
            rate_hz: Some(2.0), // one sample per 0.5 s of sim time
        }));

        assert!(
            subs.should_send_sample("p", 1, 0.0),
            "the first sample always goes"
        );
        assert!(
            !subs.should_send_sample("p", 1, 0.1),
            "0.1 s < 0.5 s ⇒ dropped"
        );
        assert!(
            !subs.should_send_sample("p", 1, 0.4),
            "still inside the period"
        );
        assert!(subs.should_send_sample("p", 1, 0.6), "0.6 s ≥ 0.5 s ⇒ sent");
    }

    /// Decimation is keyed by (name, entity). Two rovers both reporting "motor_current"
    /// must not throttle each other — that would silently halve one vehicle's telemetry.
    #[test]
    fn two_entities_with_the_same_channel_name_do_not_throttle_each_other() {
        let mut subs = TelemetrySubscriptions::default();
        subs.subscribe(Some(TelemetryFilter {
            names: vec![],
            min_severity: None,
            rate_hz: Some(2.0),
        }));
        assert!(subs.should_send_sample("motor_current", 1, 0.0));
        assert!(
            subs.should_send_sample("motor_current", 2, 0.0),
            "a DIFFERENT entity's channel of the same name must send on its own schedule"
        );
        assert!(!subs.should_send_sample("motor_current", 1, 0.1));
    }

    /// A subscriber asking for everything (no cap) must not be throttled by one that asked
    /// for a slow rate — delivery is one shared stream, so the gate is the FASTEST ask.
    #[test]
    fn an_uncapped_subscriber_is_not_throttled_by_a_slow_one() {
        let mut subs = TelemetrySubscriptions::default();
        subs.subscribe(Some(TelemetryFilter {
            names: vec![],
            min_severity: None,
            rate_hz: Some(0.1),
        }));
        subs.subscribe(Some(TelemetryFilter {
            names: vec![],
            min_severity: None,
            rate_hz: None,
        }));
        assert!(subs.should_send_sample("p", 1, 0.0));
        assert!(
            subs.should_send_sample("p", 1, 0.001),
            "the uncapped subscriber wins"
        );
    }

    /// A nonsense cap must not mute a channel forever.
    #[test]
    fn a_nonsense_rate_cap_is_treated_as_no_cap() {
        for bad in [0.0, -1.0, f64::NAN] {
            let mut subs = TelemetrySubscriptions::default();
            subs.subscribe(Some(TelemetryFilter {
                names: vec![],
                min_severity: None,
                rate_hz: Some(bad),
            }));
            assert!(subs.should_send_sample("p", 1, 0.0));
            assert!(
                subs.should_send_sample("p", 1, 0.001),
                "rate {bad} must not mute the channel"
            );
        }
    }

    #[test]
    fn test_name_filter() {
        let mut subs = TelemetrySubscriptions::default();
        subs.subscribe(Some(TelemetryFilter {
            names: vec!["motor_temp".to_string()],
            min_severity: None,
            rate_hz: None,
        }));
        assert!(subs.should_broadcast("motor_temp", None));
        assert!(!subs.should_broadcast("other_param", None));
    }
}
