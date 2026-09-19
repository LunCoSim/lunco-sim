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
use lunco_api::{
    api_param_array, api_param_f64, api_param_str, api_param_u64, ApiQueryError, ApiQueryResult,
};
use lunco_api_core::ApiErrorCode;
use lunco_api_core::{api_value, ApiValue};
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

/// Resolve the stable owner captured by the shared signal registry, falling back to
/// the live ECS component for older/session-local entries. A missing `GlobalEntityId`
/// is not zero: zero is an invalid placeholder that collapses all local physics/model
/// signals with the same name into one API key.
fn channel_owner(world: &World, signals: &SignalRegistry, signal: &SignalRef) -> ChannelOwner {
    signals
        .global_owner(signal)
        .or_else(|| world.get::<GlobalEntityId>(signal.entity).copied())
        .map(ChannelOwner::Api)
        .unwrap_or(ChannelOwner::Session(signal.entity))
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

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let signals = world.resource::<SignalRegistry>();

        let mut channels: Vec<ApiValue> = signals
            .iter_scalar()
            .map(|(sig, history)| {
                let owner = channel_owner(world, signals, sig);
                let meta = signals.meta(sig);
                let presentation = meta
                    .map(|meta| lunco_api_core::api_value_from_serializable(&meta.presentation))
                    .transpose()?;
                Ok(api_value!({
                    "key": channel_key(owner, &sig.path),
                    "name": sig.path.clone(),
                    "source": owner.api_id(),
                    "owner": match owner {
                        ChannelOwner::Api(id) => api_value!({
                            "kind": "api",
                            "api_id": id.get(),
                        }),
                        ChannelOwner::Session(entity) => api_value!({
                            "kind": "session",
                            "entity_bits": entity.to_bits(),
                        }),
                    },
                    "unit": meta.and_then(|m| m.unit.clone()),
                    "description": meta.and_then(|m| m.description.clone()),
                    "provenance": meta.and_then(|m| m.provenance.clone()),
                    "group_path": meta.and_then(|m| m.group_path.clone()),
                    "model_class": meta.and_then(|m| m.model_class.clone()),
                    "model_variable": meta.and_then(|m| m.model_variable.clone()),
                    "source_asset": meta.and_then(|m| m.source_asset.clone()),
                    "canonical_name": meta.and_then(|m| m.canonical_name.clone()),
                    "presentation": presentation,
                    "exposure": meta.map(|m| match m.exposure {
                        lunco_signal::SignalExposure::Public => "public",
                        lunco_signal::SignalExposure::Internal => "internal",
                    }),
                    "active": signals.is_active(sig),
                    // What's actually retained RIGHT NOW — a client can use this to know
                    // how far back a history query can usefully reach.
                    "samples": history.len(),
                    "retention": history.capacity,
                }))
            })
            .collect::<Result<Vec<_>, ApiQueryError>>()?;

        // Stable order: a dictionary that reshuffles every poll makes a useless tree.
        channels.sort_by(|a, b| {
            a.get("key")
                .and_then(ApiValue::as_str)
                .cmp(&b.get("key").and_then(ApiValue::as_str))
        });

        let count = channels.len();
        Ok(Some(api_value!({
            "channels": channels,
            "count": count,
        })))
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

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(key) = api_param_str(params, "key") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "missing field 'key'",
            ));
        };
        let Some((owner, name)) = parse_channel_key(key) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!("malformed channel key '{key}' — expected '<owner>:<name>'"),
            ));
        };

        // Resolve the key against the retained registry that backs the native telemetry
        // window. A raw Parameter query would reintroduce duplicate declarations; the
        // registry is the authoritative catalog of history that actually exists.
        let signal = {
            let signals = world.resource::<SignalRegistry>();
            let Some(signal) = signals
                .iter_scalar()
                .map(|(signal, _)| signal)
                .find(|signal| {
                    channel_owner(world, signals, signal) == owner && signal.path == name
                })
                .cloned()
            else {
                return Err(ApiQueryError::new(
                    ApiErrorCode::EntityNotFound,
                    format!("no retained telemetry channel '{key}'"),
                ));
            };
            signal
        };

        let start = optional_f64(params, "start", f64::NEG_INFINITY, "QueryTelemetryHistory")?;
        let end = optional_f64(params, "end", f64::INFINITY, "QueryTelemetryHistory")?;
        let limit = optional_limit(params, "limit", "QueryTelemetryHistory")?;

        let epoch_jd = world.resource::<lunco_time::WorldTime>().epoch_jd;

        let signals = world.resource::<SignalRegistry>();
        let history = signals
            .scalar_history(&signal)
            .expect("signal came from the retained registry");

        let mut samples: Vec<ApiValue> = history
            .iter()
            .filter(|s| s.time >= start && s.time <= end)
            .map(|s| api_value!({ "t": s.time, "v": s.value }))
            .collect();

        if let Some(limit) = limit {
            if samples.len() > limit {
                // Keep the NEWEST — see the doc comment.
                samples.drain(..samples.len() - limit);
            }
        }

        let count = samples.len();
        Ok(Some(api_value!({
            "key": key,
            "count": count,
            // `t` is sim_secs (precise). `epoch_jd` is the absolute frame for a client
            // that wants wall-clock labels — see the module docs on why they are separate.
            "epoch_jd": epoch_jd,
            "samples": samples,
        })))
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

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let start = optional_f64(
            params,
            "start",
            f64::NEG_INFINITY,
            "ExportTelemetryRecording",
        )?;
        let end = optional_f64(params, "end", f64::INFINITY, "ExportTelemetryRecording")?;
        let wanted: Option<Vec<String>> = match params.get("keys") {
            None | Some(ApiValue::Unit) => None,
            Some(value) => {
                let Some(keys) = api_param_array(params, "keys") else {
                    return Err(ApiQueryError::new(
                        ApiErrorCode::DeserializationError,
                        "ExportTelemetryRecording: `keys` must be an array of strings",
                    ));
                };
                let mut parsed = Vec::with_capacity(keys.len());
                for key in keys {
                    let ApiValue::Str(key) = key else {
                        return Err(ApiQueryError::new(
                            ApiErrorCode::DeserializationError,
                            "ExportTelemetryRecording: `keys` must contain only strings",
                        ));
                    };
                    parsed.push(key.clone());
                }
                let _ = value;
                Some(parsed)
            }
        };

        let signals = world.resource::<SignalRegistry>();
        let channels: Vec<(String, SignalRef)> = signals
            .iter_scalar()
            .map(|(signal, _)| {
                (
                    channel_key(channel_owner(world, signals, signal), &signal.path),
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

        let mut series = Vec::new();
        for (key, pts) in &per_key {
            let mut col: Vec<ApiValue> = Vec::with_capacity(times.len());
            let mut i = 0usize;
            for t in &times {
                // `pts` is time-ordered (a ring buffer is), so one pass walks both.
                if i < pts.len() && pts[i].0 == *t {
                    col.push(ApiValue::Float(pts[i].1));
                    i += 1;
                } else {
                    // Never sampled at this instant. `null`, not an interpolation.
                    col.push(ApiValue::Unit);
                }
            }
            series.push((key.clone(), ApiValue::Array(col)));
        }

        let count = times.len();
        Ok(Some(api_value!({
            "times": times,
            "series": ApiValue::map(series),
            "count": count,
        })))
    }
}

fn optional_f64(
    params: &ApiValue,
    name: &str,
    default: f64,
    query: &str,
) -> Result<f64, ApiQueryError> {
    match params.get(name) {
        None => Ok(default),
        Some(_) => api_param_f64(params, name).ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!("{query}: `{name}` must be a number"),
            )
        }),
    }
}

fn optional_limit(
    params: &ApiValue,
    name: &str,
    query: &str,
) -> Result<Option<usize>, ApiQueryError> {
    match params.get(name) {
        None => Ok(None),
        Some(_) => api_param_u64(params, name)
            .and_then(|value| usize::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    format!("{query}: `{name}` must be a platform-sized unsigned integer"),
                )
            }),
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

        let Ok(Some(data)) = ListTelemetryChannelsProvider.execute(&world, &ApiValue::Unit) else {
            panic!("list provider must return a catalog");
        };
        let Some(ApiValue::Array(channels)) = data.get("channels") else {
            panic!("channels array");
        };
        let keys: Vec<&str> = channels
            .iter()
            .map(|channel| {
                channel
                    .get("key")
                    .and_then(ApiValue::as_str)
                    .expect("channel key")
            })
            .collect();
        assert_eq!(keys.len(), 2);
        assert_ne!(keys[0], keys[1]);
        assert!(keys.iter().all(|key| key.starts_with("session/")));
    }

    #[test]
    fn list_provider_exposes_modelica_component_identity() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let signal = SignalRef::new(entity, "science_power");
        let mut registry = SignalRegistry::default();
        registry.push_scalar(signal.clone(), 0.0, 120.0);
        registry.update_meta(
            signal,
            lunco_signal::SignalMeta {
                model_class: Some("LunCo.Electrical.CameraPayload".into()),
                model_variable: Some("power_draw_w".into()),
                source_asset: Some("lunco://models/LunCo/Electrical/CameraPayload.mo".into()),
                canonical_name: Some("science_power".into()),
                presentation: lunco_signal::SignalPresentation::Summary {
                    group: "power".into(),
                    label: "total".into(),
                    formula: "sum of measured channels".into(),
                },
                ..Default::default()
            },
        );
        world.insert_resource(registry);

        let Ok(Some(data)) = ListTelemetryChannelsProvider.execute(&world, &ApiValue::Unit) else {
            panic!("list provider must return a catalog");
        };
        let Some(ApiValue::Array(channels)) = data.get("channels") else {
            panic!("channels array");
        };
        let channel = &channels[0];
        assert_eq!(
            channel.get("model_class").and_then(ApiValue::as_str),
            Some("LunCo.Electrical.CameraPayload")
        );
        assert_eq!(
            channel.get("model_variable").and_then(ApiValue::as_str),
            Some("power_draw_w")
        );
        assert_eq!(
            channel.get("source_asset").and_then(ApiValue::as_str),
            Some("lunco://models/LunCo/Electrical/CameraPayload.mo")
        );
        assert_eq!(
            channel.get("canonical_name").and_then(ApiValue::as_str),
            Some("science_power")
        );
        assert_eq!(
            channel
                .get("presentation")
                .and_then(|value| value.get("kind"))
                .and_then(ApiValue::as_str),
            Some("summary")
        );
        assert_eq!(
            channel
                .get("presentation")
                .and_then(|value| value.get("group"))
                .and_then(ApiValue::as_str),
            Some("power")
        );
        assert_eq!(
            channel
                .get("presentation")
                .and_then(|value| value.get("formula"))
                .and_then(ApiValue::as_str),
            Some("sum of measured channels")
        );
    }

    #[test]
    fn archived_api_channel_keeps_its_key_and_history() {
        let mut world = World::new();
        world.insert_resource(lunco_time::WorldTime::default());
        let entity = world.spawn(GlobalEntityId::from_raw(42)).id();
        let signal = SignalRef::new(entity, "motor_current");
        let mut registry = SignalRegistry::default();
        registry.push_scalar(signal.clone(), 12.0, 3.5);
        registry.associate_global_owner(&signal, GlobalEntityId::from_raw(42));
        registry.deactivate_entity(entity);
        world.insert_resource(registry);
        world.despawn(entity);

        let Ok(Some(data)) = ListTelemetryChannelsProvider.execute(&world, &ApiValue::Unit) else {
            panic!("list provider must return archived channels");
        };
        let Some(ApiValue::Array(channels)) = data.get("channels") else {
            panic!("channels array");
        };
        assert_eq!(
            channels[0].get("key").and_then(ApiValue::as_str),
            Some("api/42:motor_current")
        );
        assert_eq!(channels[0].get("active"), Some(&ApiValue::Bool(false)));

        let history = QueryTelemetryHistoryProvider
            .execute(&world, &api_value!({ "key": "api/42:motor_current" }));
        let Ok(Some(data)) = history else {
            panic!("archived API channel history must remain queryable");
        };
        let Some(ApiValue::Array(samples)) = data.get("samples") else {
            panic!("history samples must be an array");
        };
        assert_eq!(samples[0].get("t").and_then(ApiValue::as_f64), Some(12.0));
        assert_eq!(samples[0].get("v").and_then(ApiValue::as_f64), Some(3.5));

        let recording = ExportTelemetryRecordingProvider
            .execute(&world, &api_value!({ "keys": ["api/42:motor_current"] }));
        let Ok(Some(data)) = recording else {
            panic!("archived API channel export must remain addressable");
        };
        let Some(ApiValue::Array(series)) = data.get("series").and_then(|value| match value {
            ApiValue::Map(entries) => entries
                .iter()
                .find(|(key, _)| key == "api/42:motor_current")
                .map(|(_, value)| value),
            _ => None,
        }) else {
            panic!("recording series must have one array for the selected key");
        };
        assert_eq!(series[0].as_f64(), Some(3.5));
    }

    #[test]
    fn a_malformed_key_is_rejected_not_guessed() {
        assert_eq!(parse_channel_key("motor_current"), None);
        assert_eq!(parse_channel_key("api/notanumber:x"), None);
        assert_eq!(parse_channel_key("0:contact"), None);
    }
}
