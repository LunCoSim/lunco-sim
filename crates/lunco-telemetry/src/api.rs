//! The telemetry QUERY surface — catalog + history.
//!
//! Subscription (`SubscribeTelemetry`) gives you the *live* stream. That is only one of
//! the three things a real telemetry client needs. OpenMCT — and any ground-system UI
//! shaped like it — asks for exactly three:
//!
//! 1. **A dictionary**: what channels exist, what are they called, what units, what type.
//!    → [`ListTelemetryChannelsProvider`]
//! 2. **History**: give me channel K between t0 and t1 (for the plot you just opened,
//!    scrolled back, or zoomed into).
//!    → [`QueryTelemetryHistoryProvider`]
//! 3. **Realtime**: push me new values as they happen.
//!    → already exists: `SubscribeTelemetry` + `sampled_param_observer`.
//!
//! Only (3) existed. A client could subscribe to a firehose but could not ask *what is
//! there* or *what already happened* — so every plot would start empty and stay blind to
//! anything before the moment you connected. These two providers close that, and they are
//! deliberately transport-agnostic: an HTTP/WebSocket adapter (OpenMCT's telemetry
//! provider API, a YAMCS bridge) is a thin adapter over them, not a rewrite.
//!
//! # The channel key
//!
//! A channel is identified by `"<owner>:<name>"` — **not** by name alone. Names are not
//! unique: two rovers both report `"motor_current"`. The owner is typed because not every
//! signal belongs to a network-addressable entity: `api/<GlobalEntityId>` names an API
//! entity, while `session/<Entity::to_bits()>` names a local physics/model entity for the
//! lifetime of this process. This is the same `(SignalRef::entity, SignalRef::path)`
//! identity the native telemetry window uses; there is no second channel catalog.
//!
//! # The timebase
//!
//! Times are `sim_secs` — seconds on the channel's own time domain — **not** the Julian
//! Date `timestamp`. JD is ~2.46e6, leaving an `f64` about 86 µs of resolution, so a plot
//! axis built on it would quantise into visible stair-steps and any range query would be
//! sloppy at the edges. Each response also carries the absolute `epoch_jd` so a client
//! that needs wall-clock can still label its axis.

use bevy::prelude::*;
use lunco_api::queries::ApiQueryProvider;
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_core::GlobalEntityId;
use lunco_signal::{SignalRef, SignalRegistry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelOwner {
    /// Stable entity identity shared with the command API.
    Api(GlobalEntityId),
    /// Session-local identity for a signal whose producer is deliberately not networked.
    Session(Entity),
}

impl ChannelOwner {
    fn key_prefix(self) -> String {
        match self {
            Self::Api(id) => format!("api/{}", id.get()),
            Self::Session(entity) => format!("session/{}", entity.to_bits()),
        }
    }

    fn api_id(self) -> Option<u64> {
        match self {
            Self::Api(id) => Some(id.get()),
            Self::Session(_) => None,
        }
    }
}

/// Resolve the same owner identity used by the shared signal registry. A missing
/// `GlobalEntityId` is not zero: zero is an invalid placeholder that collapses all
/// local physics/model signals with the same name into one API key.
fn channel_owner(world: &World, entity: Entity) -> ChannelOwner {
    world
        .get::<GlobalEntityId>(entity)
        .copied()
        .map(ChannelOwner::Api)
        .unwrap_or(ChannelOwner::Session(entity))
}

fn channel_key(owner: ChannelOwner, name: &str) -> String {
    format!("{}:{name}", owner.key_prefix())
}

/// Split a `"<owner>:<name>"` key. The name may itself contain `:`, so split ONCE.
fn parse_channel_key(key: &str) -> Option<(ChannelOwner, &str)> {
    let (owner, name) = key.split_once(':')?;
    let (kind, raw) = owner.split_once('/')?;
    let owner = match kind {
        "api" => ChannelOwner::Api(GlobalEntityId::from_raw(raw.parse().ok()?)),
        "session" => ChannelOwner::Session(Entity::from_bits(raw.parse().ok()?)),
        _ => return None,
    };
    Some((owner, name))
}

/// The dictionary: every retained signal in the shared [`SignalRegistry`]. The native
/// telemetry window and this API therefore see the same channel set; raw `Parameter`
/// declarations are policy inputs, not a second catalog.
pub(crate) struct ListTelemetryChannelsProvider;

impl ApiQueryProvider for ListTelemetryChannelsProvider {
    fn name(&self) -> &'static str {
        "ListTelemetryChannels"
    }

    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> ApiResponse {
        let signals = world.resource::<SignalRegistry>();

        let mut channels: Vec<serde_json::Value> = signals
            .iter_scalar()
            .map(|(sig, history)| {
                let owner = channel_owner(world, sig.entity);
                let meta = signals.meta(sig);
                serde_json::json!({
                    "key": channel_key(owner, &sig.path),
                    "name": sig.path,
                    "source": owner.api_id(),
                    "owner": match owner {
                        ChannelOwner::Api(id) => serde_json::json!({
                            "kind": "api",
                            "api_id": id.get(),
                        }),
                        ChannelOwner::Session(entity) => serde_json::json!({
                            "kind": "session",
                            "entity_bits": entity.to_bits(),
                        }),
                    },
                    "unit": meta.and_then(|m| m.unit.clone()),
                    "description": meta.and_then(|m| m.description.clone()),
                    "provenance": meta.and_then(|m| m.provenance.clone()),
                    "group_path": meta.and_then(|m| m.group_path.clone()),
                    "exposure": meta.map(|m| match m.exposure {
                        lunco_signal::SignalExposure::Public => "public",
                        lunco_signal::SignalExposure::Internal => "internal",
                    }),
                    "active": signals.is_active(sig),
                    // What's actually retained RIGHT NOW — a client can use this to know
                    // how far back a history query can usefully reach.
                    "samples": history.len(),
                    "retention": history.capacity,
                })
            })
            .collect();

        // Stable order: a dictionary that reshuffles every poll makes a useless tree.
        channels.sort_by(|a, b| {
            a["key"]
                .as_str()
                .unwrap_or("")
                .cmp(b["key"].as_str().unwrap_or(""))
        });

        ApiResponse::ok(serde_json::json!({
            "channels": channels,
            "count": channels.len(),
        }))
    }
}

/// History: the retained samples of one channel, optionally windowed.
///
/// Params: `{ "key": "<owner>:<name>", "start": <sim_secs>?, "end": <sim_secs>?,
///            "limit": <usize>? }`
///
/// `start`/`end` are inclusive bounds on `sim_secs`; omit either for "unbounded on that
/// side". `limit` keeps the MOST RECENT n samples of the window — a plot that asks for a
/// bounded number of points wants the newest ones, not a truncated prefix ending in the
/// distant past.
pub(crate) struct QueryTelemetryHistoryProvider;

impl ApiQueryProvider for QueryTelemetryHistoryProvider {
    fn name(&self) -> &'static str {
        "QueryTelemetryHistory"
    }

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let Some(key) = params.get("key").and_then(|v| v.as_str()) else {
            return ApiResponse::error(ApiErrorCode::DeserializationError, "missing field 'key'");
        };
        let Some((owner, name)) = parse_channel_key(key) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                format!("malformed channel key '{key}' — expected '<owner>:<name>'"),
            );
        };

        // Resolve the key against the retained registry that backs the native telemetry
        // window. A raw Parameter query would reintroduce duplicate declarations; the
        // registry is the authoritative catalog of history that actually exists.
        let signal = {
            let signals = world.resource::<SignalRegistry>();
            let Some(signal) = signals
                .iter_scalar()
                .map(|(signal, _)| signal)
                .find(|signal| channel_owner(world, signal.entity) == owner && signal.path == name)
                .cloned()
            else {
                return ApiResponse::error(
                    ApiErrorCode::EntityNotFound,
                    format!("no retained telemetry channel '{key}'"),
                );
            };
            signal
        };

        let start = params
            .get("start")
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::NEG_INFINITY);
        let end = params
            .get("end")
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::INFINITY);
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        let epoch_jd = world.resource::<lunco_time::WorldTime>().epoch_jd;

        let signals = world.resource::<SignalRegistry>();
        let history = signals
            .scalar_history(&signal)
            .expect("signal came from the retained registry");

        let mut samples: Vec<serde_json::Value> = history
            .iter()
            .filter(|s| s.time >= start && s.time <= end)
            .map(|s| serde_json::json!({ "t": s.time, "v": s.value }))
            .collect();

        if let Some(limit) = limit {
            if samples.len() > limit {
                // Keep the NEWEST — see the doc comment.
                samples.drain(..samples.len() - limit);
            }
        }

        ApiResponse::ok(serde_json::json!({
            "key": key,
            "count": samples.len(),
            // `t` is sim_secs (precise). `epoch_jd` is the absolute frame for a client
            // that wants wall-clock labels — see the module docs on why they are separate.
            "epoch_jd": epoch_jd,
            "samples": samples,
        }))
    }
}

/// Export a set of channels as a **recording** — the columnar shape experiments already
/// produce and plots already consume.
///
/// Params: `{ "keys": ["<owner>:<name>", …]?, "start": <sim_secs>?, "end": <sim_secs>? }`
/// (omit `keys` for every channel).
///
/// Returns `{ times: [t…], series: { key: [v…] } }` — the same shape as
/// `lunco_experiments::RunResult { times, series }`, so an experiments plot, a CSV export,
/// or a comparison against a Modelica run can consume a telemetry recording without a
/// second code path.
///
/// # There is no separate recorder
///
/// A "recording" is not a mode you start and stop with its own buffer — **the ring buffer
/// IS the recording.** Channels are already retained at their own depth; exporting is a
/// read. A start/stop recorder would be a second store holding the same samples, with its
/// own retention bug waiting to happen.
///
/// # The union time grid
///
/// Channels sample at *different rates* (that is the point of Phase 1), so they do not
/// share a time axis. The export builds the sorted union of every sample time and fills a
/// channel's missing slots with `null` — the same NaN-padding `RunResult::merge_delta`
/// does when a run discovers a new variable mid-flight. **Do not interpolate here**: a
/// hole is data the channel genuinely never reported, and inventing a value would launder
/// a 1 Hz channel into looking like a 60 Hz one.
pub(crate) struct ExportTelemetryRecordingProvider;

impl ApiQueryProvider for ExportTelemetryRecordingProvider {
    fn name(&self) -> &'static str {
        "ExportTelemetryRecording"
    }

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let start = params
            .get("start")
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::NEG_INFINITY);
        let end = params
            .get("end")
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::INFINITY);
        let wanted: Option<Vec<String>> = params.get("keys").and_then(|v| v.as_array()).map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        });

        let signals = world.resource::<SignalRegistry>();
        let channels: Vec<(String, SignalRef)> = signals
            .iter_scalar()
            .map(|(signal, _)| {
                (
                    channel_key(channel_owner(world, signal.entity), &signal.path),
                    signal.clone(),
                )
            })
            .filter(|(key, _)| wanted.as_ref().is_none_or(|w| w.contains(key)))
            .collect();

        // Collect each channel's (t, v) inside the window.
        let mut per_key: Vec<(String, Vec<(f64, f64)>)> = Vec::new();
        for (key, sig) in channels {
            let pts: Vec<(f64, f64)> = signals
                .scalar_history(&sig)
                .map(|h| {
                    h.iter()
                        .filter(|s| s.time >= start && s.time <= end)
                        .map(|s| (s.time, s.value))
                        .collect()
                })
                .unwrap_or_default();
            per_key.push((key, pts));
        }

        // The union time grid — channels at different rates share no axis of their own.
        let mut times: Vec<f64> = per_key
            .iter()
            .flat_map(|(_, p)| p.iter().map(|(t, _)| *t))
            .collect();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        times.dedup();

        let mut series = serde_json::Map::new();
        for (key, pts) in &per_key {
            let mut col: Vec<serde_json::Value> = Vec::with_capacity(times.len());
            let mut i = 0usize;
            for t in &times {
                // `pts` is time-ordered (a ring buffer is), so one pass walks both.
                if i < pts.len() && pts[i].0 == *t {
                    col.push(serde_json::json!(pts[i].1));
                    i += 1;
                } else {
                    // Never sampled at this instant. `null`, not an interpolation.
                    col.push(serde_json::Value::Null);
                }
            }
            series.insert(key.clone(), serde_json::Value::Array(col));
        }

        ApiResponse::ok(serde_json::json!({
            "times": times,
            "series": series,
            "count": times.len(),
        }))
    }
}

pub(crate) fn build(app: &mut App) {
    // `init_resource` first: plugin order is not ours to control, and `resource_mut` on a
    // registry lunco-api hasn't installed yet would panic.
    app.init_resource::<lunco_api::queries::ApiQueryRegistry>();
    let mut registry = app
        .world_mut()
        .resource_mut::<lunco_api::queries::ApiQueryRegistry>();
    registry.register(ListTelemetryChannelsProvider);
    registry.register(QueryTelemetryHistoryProvider);
    registry.register(ExportTelemetryRecordingProvider);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_key_round_trips() {
        let k = channel_key(
            ChannelOwner::Api(GlobalEntityId::from_raw(42)),
            "motor_current",
        );
        assert_eq!(k, "api/42:motor_current");
        assert_eq!(
            parse_channel_key(&k),
            Some((
                ChannelOwner::Api(GlobalEntityId::from_raw(42)),
                "motor_current"
            ))
        );
    }

    /// A name containing a colon must not corrupt the key — split once, not greedily.
    #[test]
    fn a_name_with_a_colon_survives_the_key() {
        let k = channel_key(
            ChannelOwner::Api(GlobalEntityId::from_raw(7)),
            "bus:voltage",
        );
        assert_eq!(
            parse_channel_key(&k),
            Some((
                ChannelOwner::Api(GlobalEntityId::from_raw(7)),
                "bus:voltage"
            ))
        );
    }

    #[test]
    fn a_local_owner_is_distinct_from_every_other_local_owner() {
        let a = Entity::from_raw_u32(10).unwrap();
        let b = Entity::from_raw_u32(11).unwrap();
        let a_key = channel_key(ChannelOwner::Session(a), "contact");
        let b_key = channel_key(ChannelOwner::Session(b), "contact");
        assert_ne!(a_key, b_key);
        assert_eq!(
            parse_channel_key(&a_key),
            Some((ChannelOwner::Session(a), "contact"))
        );
    }

    #[test]
    fn list_provider_keeps_same_named_local_signals_separate() {
        let mut world = World::new();
        let left = world.spawn_empty().id();
        let right = world.spawn_empty().id();
        let mut registry = SignalRegistry::default();
        registry.push_scalar(SignalRef::new(left, "contact"), 0.0, 1.0);
        registry.push_scalar(SignalRef::new(right, "contact"), 0.0, 0.0);
        world.insert_resource(registry);

        let response = ListTelemetryChannelsProvider.execute(&mut world, &serde_json::Value::Null);
        let ApiResponse::Ok {
            data: Some(data), ..
        } = response
        else {
            panic!("list provider must return a catalog");
        };
        let keys: Vec<&str> = data["channels"]
            .as_array()
            .expect("channels array")
            .iter()
            .map(|channel| channel["key"].as_str().expect("channel key"))
            .collect();
        assert_eq!(keys.len(), 2);
        assert_ne!(keys[0], keys[1]);
        assert!(keys.iter().all(|key| key.starts_with("session/")));
    }

    #[test]
    fn a_malformed_key_is_rejected_not_guessed() {
        assert_eq!(parse_channel_key("motor_current"), None);
        assert_eq!(parse_channel_key("api/notanumber:x"), None);
        assert_eq!(parse_channel_key("0:contact"), None);
    }
}
