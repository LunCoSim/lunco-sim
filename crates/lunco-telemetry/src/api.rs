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
//! provider API, a YAMCS bridge) is a thin shim over them, not a rewrite.
//!
//! # The channel key
//!
//! A channel is identified by `"<api_id>:<name>"` — **not** by name alone. Names are not
//! unique: two rovers both report `"motor_current"`. OpenMCT wants one opaque, stable
//! string per telemetry point, and this is it. `api_id` is the same `GlobalEntityId` the
//! rest of the API speaks, so a client can go from a telemetry point back to the entity
//! that owns it.
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
use lunco_core::telemetry::Parameter;
use lunco_core::GlobalEntityId;
use lunco_signal::{SignalRef, SignalRegistry};

/// `"<api_id>:<name>"`. See the module docs on why the entity is part of the key.
fn channel_key(gid: u64, name: &str) -> String {
    format!("{gid}:{name}")
}

/// Split a `"<api_id>:<name>"` key. The name may itself contain `:`, so split ONCE.
fn parse_channel_key(key: &str) -> Option<(u64, &str)> {
    let (gid, name) = key.split_once(':')?;
    Some((gid.parse().ok()?, name))
}

/// The dictionary: every telemetry channel that exists, with enough metadata for a client
/// to build a tree and a plot axis without guessing.
pub(crate) struct ListTelemetryChannelsProvider;

impl ApiQueryProvider for ListTelemetryChannelsProvider {
    fn name(&self) -> &'static str {
        "ListTelemetryChannels"
    }

    fn execute(&self, world: &mut World, _params: &serde_json::Value) -> ApiResponse {
        let signals = world.get_resource::<SignalRegistry>().map(|s| {
            s.iter_scalar()
                .map(|(r, h)| (r.clone(), h.len(), h.capacity))
                .collect::<Vec<_>>()
        });
        let sample_counts: std::collections::HashMap<SignalRef, (usize, usize)> = signals
            .unwrap_or_default()
            .into_iter()
            .map(|(r, len, cap)| (r, (len, cap)))
            .collect();

        // A channel may be its own entity targeting what it measures, and only the MEASURED
        // entity carries a `GlobalEntityId` — so resolve the id through the target, not
        // through the entity the component happens to sit on.
        let gids: std::collections::HashMap<Entity, u64> = world
            .query::<(Entity, &GlobalEntityId)>()
            .iter(world)
            .map(|(e, g)| (e, g.get()))
            .collect();

        let raw: Vec<(Entity, Parameter)> = world
            .query::<(Entity, &Parameter)>()
            .iter(world)
            .map(|(e, p)| (e, p.clone()))
            .collect();

        let mut channels: Vec<serde_json::Value> = raw
            .into_iter()
            .map(|(entity, p)| {
                let measured = p.target.unwrap_or(entity);
                let gid = gids.get(&measured).copied().unwrap_or(0);
                let sig = SignalRef::new(measured, p.name.clone());
                let (samples, capacity) = sample_counts.get(&sig).copied().unwrap_or((0, 0));
                serde_json::json!({
                    "key": channel_key(gid, &p.name),
                    "name": p.name,
                    "source": gid,
                    "unit": p.unit,
                    "enabled": p.enabled,
                    "rate_hz": p.rate_hz,
                    "deadband": p.deadband,
                    // What's actually retained RIGHT NOW — a client can use this to know
                    // how far back a history query can usefully reach.
                    "samples": samples,
                    "retention": capacity,
                })
            })
            .collect();

        // Stable order: a dictionary that reshuffles every poll makes a useless tree.
        channels.sort_by(|a, b| {
            a["key"].as_str().unwrap_or("").cmp(b["key"].as_str().unwrap_or(""))
        });

        ApiResponse::ok(serde_json::json!({
            "channels": channels,
            "count": channels.len(),
        }))
    }
}

/// History: the retained samples of one channel, optionally windowed.
///
/// Params: `{ "key": "<api_id>:<name>", "start": <sim_secs>?, "end": <sim_secs>?,
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
        let Some((gid, name)) = parse_channel_key(key) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                format!("malformed channel key '{key}' — expected '<api_id>:<name>'"),
            );
        };

        // Resolve the key back to the MEASURED entity. A channel key is stable; the entity
        // behind it is not (a reloaded twin re-mints entities), so this is a lookup, not a
        // cached handle. And the channel component may live on a different entity than the one
        // carrying the `GlobalEntityId` — match through `target`.
        let gids: std::collections::HashMap<Entity, u64> = world
            .query::<(Entity, &GlobalEntityId)>()
            .iter(world)
            .map(|(e, g)| (e, g.get()))
            .collect();
        let entity = world
            .query::<(Entity, &Parameter)>()
            .iter(world)
            .map(|(e, p)| (p.target.unwrap_or(e), p.name.clone()))
            .find(|(measured, n)| gids.get(measured).copied() == Some(gid) && n == name)
            .map(|(measured, _)| measured);
        let Some(entity) = entity else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("no live telemetry channel '{key}'"),
            );
        };

        let start = params.get("start").and_then(|v| v.as_f64()).unwrap_or(f64::NEG_INFINITY);
        let end = params.get("end").and_then(|v| v.as_f64()).unwrap_or(f64::INFINITY);
        let limit = params.get("limit").and_then(|v| v.as_u64()).map(|n| n as usize);

        let epoch_jd = world
            .get_resource::<lunco_time::WorldTime>()
            .map(|w| w.epoch_jd)
            .unwrap_or(0.0);

        let Some(signals) = world.get_resource::<SignalRegistry>() else {
            return ApiResponse::ok(serde_json::json!({ "key": key, "samples": [] }));
        };
        let sig = SignalRef::new(entity, name.to_string());
        let Some(history) = signals.scalar_history(&sig) else {
            // The channel exists but has produced nothing yet (just authored, or
            // deadband-suppressed). Empty is a valid answer, not an error — a client
            // must be able to tell "no data" from "no such channel".
            return ApiResponse::ok(serde_json::json!({
                "key": key, "samples": [], "count": 0, "epoch_jd": epoch_jd,
            }));
        };

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
/// Params: `{ "keys": ["<api_id>:<name>", …]?, "start": <sim_secs>?, "end": <sim_secs>? }`
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
        let start = params.get("start").and_then(|v| v.as_f64()).unwrap_or(f64::NEG_INFINITY);
        let end = params.get("end").and_then(|v| v.as_f64()).unwrap_or(f64::INFINITY);
        let wanted: Option<Vec<String>> = params.get("keys").and_then(|v| v.as_array()).map(|a| {
            a.iter().filter_map(|x| x.as_str().map(String::from)).collect()
        });

        // key -> entity, for every live channel.
        let gids: std::collections::HashMap<Entity, u64> = world
            .query::<(Entity, &GlobalEntityId)>()
            .iter(world)
            .map(|(e, g)| (e, g.get()))
            .collect();
        let channels: Vec<(String, Entity, String)> = world
            .query::<(Entity, &Parameter)>()
            .iter(world)
            .map(|(e, p)| {
                let measured = p.target.unwrap_or(e);
                let gid = gids.get(&measured).copied().unwrap_or(0);
                (channel_key(gid, &p.name), measured, p.name.clone())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .filter(|(key, _, _)| wanted.as_ref().is_none_or(|w| w.contains(key)))
            .collect();

        let Some(signals) = world.get_resource::<SignalRegistry>() else {
            return ApiResponse::ok(serde_json::json!({ "times": [], "series": {} }));
        };

        // Collect each channel's (t, v) inside the window.
        let mut per_key: Vec<(String, Vec<(f64, f64)>)> = Vec::new();
        for (key, entity, name) in channels {
            let sig = SignalRef::new(entity, name);
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
        let mut times: Vec<f64> = per_key.iter().flat_map(|(_, p)| p.iter().map(|(t, _)| *t)).collect();
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
    let mut registry = app.world_mut().resource_mut::<lunco_api::queries::ApiQueryRegistry>();
    registry.register(ListTelemetryChannelsProvider);
    registry.register(QueryTelemetryHistoryProvider);
    registry.register(ExportTelemetryRecordingProvider);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_key_round_trips() {
        let k = channel_key(42, "motor_current");
        assert_eq!(k, "42:motor_current");
        assert_eq!(parse_channel_key(&k), Some((42, "motor_current")));
    }

    /// A name containing a colon must not corrupt the key — split once, not greedily.
    #[test]
    fn a_name_with_a_colon_survives_the_key() {
        let k = channel_key(7, "bus:voltage");
        assert_eq!(parse_channel_key(&k), Some((7, "bus:voltage")));
    }

    #[test]
    fn a_malformed_key_is_rejected_not_guessed() {
        assert_eq!(parse_channel_key("motor_current"), None);
        assert_eq!(parse_channel_key("notanumber:x"), None);
    }
}
