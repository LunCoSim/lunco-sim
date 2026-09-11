//! UI-facing bridges from simulator state to the LunCoSim presentation layer.
//!
//! These systems are application integration, not simulation mechanics. Keeping
//! them in the existing UI package means status, camera, terrain-shadow, and
//! environment presentation edits do not rebuild the headless composition root.

use crate::terrain_horizon;
use bevy::prelude::*;
use lunco_usd_bevy_core::{UsdRead, UsdStageAsset};

pub(crate) fn register(app: &mut App) {
    app.init_resource::<TerrainStatusMirrorState>().add_systems(
        lunco_core::SceneTeardown,
        reset_terrain_status_mirror_on_scene_teardown,
    );
    app.init_resource::<ModelicaStatusMirrorState>()
        .add_systems(
            lunco_core::SceneTeardown,
            reset_modelica_status_mirror_on_scene_teardown,
        );
    app.add_systems(
        Update,
        (
            report_terrain_stream_status,
            report_terrain_generation_status,
        )
            .run_if(resource_exists::<lunco_status_core::status_bus::StatusBus>),
    );
    app.add_systems(
        Update,
        report_scene_spawn_status
            .after(lunco_usd_bevy::process_queued_usd_visuals)
            .before(lunco_workbench::screenshot::OfflineRecordingReadinessSet)
            .run_if(resource_exists::<lunco_status_core::status_bus::StatusBus>),
    );
    app.add_systems(
        PostUpdate,
        report_dome_environment_status
            .run_if(resource_exists::<lunco_status_core::status_bus::StatusBus>),
    );
    app.add_systems(
        Update,
        report_modelica_status.run_if(resource_exists::<lunco_status_core::status_bus::StatusBus>),
    );
    app.add_systems(
        Update,
        start_camera_paths_when_recording_starts
            .run_if(resource_exists::<lunco_workbench::screenshot::OfflineRecordingState>),
    );
    app.add_systems(
        Update,
        mirror_recording_to_terrain_lockstep
            .before(lunco_terrain_surface::stream_viz::update_lod_tiles)
            .run_if(resource_exists::<lunco_workbench::screenshot::OfflineRecordingState>),
    );
    terrain_horizon::register(app);
    app.init_resource::<AuthoredEnv>();
    app.add_systems(lunco_core::SceneTeardown, reset_authored_env);
    app.add_systems(Update, (project_env_settings, apply_authored_env).chain());
}

/// **Environment-settings projection** — the read half of persisting
/// `SetEnvironmentLight` render knobs (exposure / bloom / ambient / earthshine)
/// onto the `LunCoEnvironment` settings prim (see
/// [`lunco_environment::LUNCO_ENVIRONMENT_PRIM_TYPE`]). On any composed-stage
/// change, read that prim's `lunco:env:*` attrs and apply them **directly** to
/// the live render state — never by re-triggering `SetEnvironmentLight`, which
/// would re-persist and loop. So a persisted render tweak round-trips on reload
/// and syncs to peers (the prim rides the USD journal → each peer recomposes →
/// each peer's projector applies) with no bespoke broadcast. Change-gated on
/// total stage generation + count, like [`project_usd_policies`]. UI-gated: the
/// knobs are render/camera state; the headless server has no cameras to apply to.
/// What the scene AUTHORED, held independently of what currently exists to
/// apply it to.
///
/// [`project_env_settings`] is change-gated on stage generation, and cameras do
/// not exist when a scene's stage first composes. Reading the prim and writing
/// the camera in one memoised pass therefore lost the value entirely: the pass
/// ran once against zero cameras, the generation never changed again, and the
/// authored exposure was never applied — the scene rendered at Bevy's default
/// EV 9.7 (~6 stops open) no matter what the USD said. Splitting the two makes
/// the read authoritative and the application idempotent.
#[derive(Resource, Default, Clone, Copy)]
pub(crate) struct AuthoredEnv {
    pub exposure_ev100: Option<f32>,
}

/// Clear scene-owned environment opinions before the next composition epoch.
///
/// `AuthoredEnv` deliberately outlives individual camera entities so cameras
/// spawned after stage composition inherit a value authored by that scene. It
/// must therefore be reset at the same lifecycle boundary as the scene, rather
/// than relying on camera despawn or on the next scene happening to author a
/// replacement value.
fn reset_authored_env(mut authored: ResMut<AuthoredEnv>) {
    *authored = AuthoredEnv::default();
}

/// Apply the authored environment exposure to every camera that exists RIGHT NOW.
///
/// Runs every frame and is a no-op when the values already match, so a camera
/// spawned (or respawned, or reparented on possession) long after the scene
/// loaded still gets the scene's exposure.
fn apply_authored_env(
    authored: Option<Res<AuthoredEnv>>,
    mut q_exposure: Query<&mut bevy::camera::Exposure>,
) {
    let Some(authored) = authored else { return };
    if let Some(ev) = authored.exposure_ev100 {
        for mut e in &mut q_exposure {
            if e.ev100 != ev {
                e.ev100 = ev;
            }
        }
    }
}

fn project_env_settings(
    stages: Res<Assets<UsdStageAsset>>,
    canonical: NonSend<lunco_usd_bevy_core::canonical::CanonicalStages>,
    roots: Query<&lunco_usd_bevy_scene::UsdPrimPath, With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    mut authored: ResMut<AuthoredEnv>,
    bloom_override: Option<ResMut<lunco_render::SceneBloomOverride>>,
    // Ambient is NOT projected here any more — it is composed from authored
    // `DomeLight` prims by `light.rs::on_usd_light_added`. See the note below.
    _ambient: Option<ResMut<bevy::light::GlobalAmbientLight>>,
    // Earthshine is likewise NOT projected here any more — it is an authored
    // light prim, loaded like every other. See the note below.
    // The exposure single-source-of-truth — see the `exposureEv100` branch.
    mut lunar_sun: Option<ResMut<lunco_environment::LunarSun>>,
    mut last: Local<Option<(usize, usize, u64)>>,
) {
    let root_ids: Vec<_> = roots.iter().map(|prim| prim.stage_handle.id()).collect();
    let signal = (
        root_ids.len(),
        root_ids.iter().filter_map(|id| stages.get(*id)).count(),
        root_ids
            .iter()
            .filter_map(|id| stages.get(*id).map(|_| canonical.generation_for(*id)))
            .sum::<u64>(),
    );
    if *last == Some(signal) {
        return;
    }
    *last = Some(signal);

    let mut scene_bloom = None;
    for stage_id in root_ids {
        let Some(stage_asset) = stages.get(stage_id) else {
            continue;
        };
        let (reader, _generation) = canonical.reader_for(stage_id, stage_asset);
        for prim in reader.prim_paths() {
            if reader.type_name(&prim).as_deref()
                != Some(lunco_environment::LUNCO_ENVIRONMENT_PRIM_TYPE)
            {
                continue;
            }
            if reader.has_authored_attribute(&prim, "lunco:env:exposureEv100") {
                if let Some(ev) = reader
                    .real_f32(&prim, "lunco:env:exposureEv100")
                    .filter(|ev| ev.is_finite())
                {
                    // RECORD it — `apply_authored_env` owns getting it onto cameras,
                    // including cameras that do not exist yet. Also seed `LunarSun`,
                    // the documented single source the sun spawn and the celestial
                    // auto-exposure both read, so a scene that later gains a
                    // celestial hierarchy ramps toward the authored value instead of
                    // the studio default.
                    authored.exposure_ev100 = Some(ev);
                    if let Some(sun) = lunar_sun.as_mut() {
                        sun.exposure_ev100 = ev;
                    }
                } else {
                    warn!("ignoring invalid authored lunco:env:exposureEv100 on {prim}");
                }
            }
            if let Some(bi) = reader.real_f32(&prim, "lunco:env:bloomIntensity") {
                if reader.has_authored_attribute(&prim, "lunco:env:bloomIntensity")
                    && bi.is_finite()
                    && bi >= 0.0
                {
                    scene_bloom = Some(bi);
                } else if reader.has_authored_attribute(&prim, "lunco:env:bloomIntensity") {
                    warn!("ignoring invalid authored lunco:env:bloomIntensity on {prim}");
                }
            }
            // `lunco:env:ambientBrightness` is DELETED, not deprecated. Uniform
            // environment illumination is already standard USD — an untextured
            // `UsdLuxDomeLight` — and `light.rs::on_usd_light_added` composes the
            // scene ambient as the sum over authored domes, which is what UsdLux
            // semantics require (lights add).
            //
            // Scenes author the bounce as a `DomeLight` prim now. There is
            // deliberately no fallback read.
            // Earthshine is not projected here either. It is an authored
            // `DistantLight` under the body it reflects from, so its brightness
            // and tint are `inputs:intensity` / `inputs:color` on that prim,
            // read by the standard light loader.
        }
    }
    if let Some(mut override_value) = bloom_override {
        if override_value.intensity != scene_bloom {
            override_value.intensity = scene_bloom;
        }
    }
}

/// **Start each camera path when the RECORDER starts.** A shot begins when the
/// camera rolls.
///
/// This replaced `start_camera_paths_when_terrain_ready`, which released on
/// "terrain resident" — an asset event on the wall clock. That was wrong twice
/// over, and the second one was expensive:
///
/// 1. **It made offline recording irreproducible.** MEASURED: two runs of
///    `episode_02_rover.usda` differed at EVERY frame of EVERY shot, starting at
///    frame 0 (viewport-crop RMSE 0.019-0.61, far above the perf-HUD text burnt
///    into each frame). A path released on a wall-clock event has already advanced
///    by an unknown amount of real time when capture begins, so the domain clock's
///    value at frame 0 was accumulated real time, not a constant. `camera_path.rs`
///    samples the curve as a pure function of that clock — pure of a floating
///    origin is still floating. Pinning the per-frame delta downstream (which the
///    recorder does) cannot fix an origin that moves.
/// 2. **It mis-framed shots.** One measured run opened on the camera pitched up at
///    empty starfield: the path had begun *before* the recorder and the opening
///    beat was simply gone. The hold existed to prevent exactly that.
///
/// Releasing from the recorder's start edge makes the gate's own time 0 at frame 0
/// by construction, after which it advances `1/fps` per captured frame — so the
/// pose at frame N is `f(N/fps)`, identical across runs and machines.
///
/// **Continuation across shots is the idempotence.** `release_camera_path_gate` is
/// a no-op on an already-running gate, and `Playback::head` keeps advancing between
/// shots, so the campaign's single 58 s curve spanning six shots stays ONE
/// continuous move — the release fires for real only on the first shot. It is not
/// rewound per shot, which would give six identical stutters instead.
///
/// **Now possible: per-shot camera paths.** Every gate used to release
/// simultaneously on one global terrain event, which is why the campaign is
/// authored as a single continuous curve. With release owned by the recorder, a
/// path could instead be bound to a specific shot and released only when that shot
/// starts. Nothing here does that yet — noted so whoever authors shots next knows
/// the constraint has lifted.
///
/// **Consequence — live preview.** Terrain-ready was also what started paths in an
/// ordinary interactive session, where no recorder ever runs; those paths would
/// otherwise stay held forever. That is now served by an EXPLICIT transport verb,
/// the `CameraPath` command
/// ([`camera_path_transport`](lunco_usd_bevy_camera::camera_path::camera_path_transport)),
/// addressed by the path prim's USD path. Still deliberately not a second
/// *automatic* release: two things racing to start the same shot is the bug this
/// replaced. One automatic start (the recorder, for capture) and one manual verb
/// (the command, for preview and scrubbing), never a fallback chain between them.
fn start_camera_paths_when_recording_starts(
    recording: Res<lunco_workbench::screenshot::OfflineRecordingState>,
    resolved: Res<lunco_time::ResolvedDomains>,
    q_paths: Query<&lunco_usd_bevy_camera::camera_path::CameraPath>,
    q_driven: Query<&lunco_usd_bevy_camera::camera_path::CameraPathDriven>,
    mut gates: Query<(
        &lunco_usd_bevy_camera::camera_path::CameraPathGate,
        &mut lunco_time::TimeDomain,
    )>,
    // Level-trigger until every authored camera path has produced a valid frame.
    // Recording and USD composition are independent lifecycles; consuming the
    // recording edge before a path has a live grid/target would permanently leave
    // that path held or capture its spawn pose.
    mut was_active: Local<bool>,
) {
    if !recording.active {
        *was_active = false;
        return;
    }
    if *was_active || gates.is_empty() {
        return;
    }

    if q_paths.iter().any(|path| {
        q_driven
            .get(path.camera)
            .map_or(true, |driven| !driven.primed)
    }) {
        return;
    }

    // Resolve every parent before releasing any gate. A partial release would
    // give paths different time origins in the same take.
    let parent_times: Vec<(Entity, f64)> = gates
        .iter()
        .map(|(gate, _)| (gate.parent, resolved.get(gate.parent)))
        .filter_map(|(parent, time)| time.map(|time| (parent, time)))
        .collect();
    if parent_times.len() != gates.iter().count() {
        return;
    }
    for (gate, mut domain) in &mut gates {
        let Some((_, parent_t)) = parent_times
            .iter()
            .find(|(parent, _)| *parent == gate.parent)
        else {
            return;
        };
        // Change-detection: `Query::iter_mut` hands out `Mut`, so touching an
        // already-running gate would mark it changed every frame. The release is
        // idempotent, but do not pay for it on a running shot.
        if domain.scale == 0.0 {
            lunco_usd_bevy_camera::camera_path::release_camera_path_gate(&mut domain, *parent_t);
            info!("[camera-path] recording started — rolling shot from its first frame");
        }
    }
    *was_active = true;
}

/// Mirror the recorder's `active` bit onto
/// [`TerrainStreamLockstep`](lunco_terrain_surface::TerrainStreamLockstep), so terrain
/// tile streaming runs in lockstep with the captured frame instead of against the
/// wall clock for exactly as long as a recording is capturing.
///
/// The problem it closes: the readiness gate makes the scene presentable at frame 0,
/// and recorder-owned camera-path release makes frame 0 bit-identical across runs —
/// but neither holds streaming steady THROUGH a shot. As the camera moves the LOD
/// selection changes, bakes are queued, and they land a scheduling-dependent number
/// of frames later. MEASURED before this: two runs of `episode_02_rover.usda`
/// differed on the frozen shots (01, 02, 03, 06) in 25-38 separate blocks of frames
/// each, with the final frame matching every time — a transient, not accumulation,
/// which is the signature of streaming catching up at a different rate.
///
/// See [`TerrainStreamLockstep`](lunco_terrain_surface::TerrainStreamLockstep) for
/// what the flag changes and why it is a flag rather than the default.
///
/// Level-triggered, not edge-triggered (unlike
/// [`start_camera_paths_when_recording_starts`], which needs an instant): the flag
/// must be true for the whole capture and false after, including after a recording
/// that ended by timing out. Writes only on an actual change so the resource's
/// change-detection tick stays meaningful.
fn mirror_recording_to_terrain_lockstep(
    recording: Res<lunco_workbench::screenshot::OfflineRecordingState>,
    mut lockstep: ResMut<lunco_terrain_surface::TerrainStreamLockstep>,
) {
    if lockstep.0 != recording.active {
        lockstep.0 = recording.active;
        info!(
            "[terrain] streaming lockstep {} (offline recording {})",
            if recording.active { "ON" } else { "OFF" },
            if recording.active { "started" } else { "ended" },
        );
    }
}

/// Mirror [`lunco_terrain_surface::TerrainStreamStatus`] into the workbench
/// [`StatusBus`](lunco_status_core::status_bus::StatusBus) so scene-open tile
/// baking is visible ("streaming terrain N/M" + progress bar) instead of an
/// unexplained black viewport. The active progress entry is the sole live
/// streaming state; once it clears, publish the current terminal count so the
/// status bar cannot fall back to an obsolete start message.
fn report_terrain_stream_status(
    status: Res<lunco_terrain_surface::TerrainStreamStatus>,
    derived: Res<lunco_terrain_surface::TerrainDerivedStatus>,
    // `Option`: the `ui` FEATURE is compile-time, but `--no-ui` headless is a
    // RUNTIME choice on the same binary — the workbench (and its `StatusBus`)
    // is simply not added there, and a bare `ResMut` panics the whole app.
    bus: Option<ResMut<lunco_status_core::status_bus::StatusBus>>,
    mut mirror: ResMut<TerrainStatusMirrorState>,
) {
    let Some(mut bus) = bus else { return };
    const STREAM_SOURCE: &str = lunco_status_core::status_bus::TERRAIN_SOURCE;
    const DERIVED_SOURCE: &str = lunco_status_core::status_bus::TERRAIN_DERIVED_SOURCE;
    // A resident count can reach the selected count before the last bake or
    // render-material publication finishes. Keep the typed live state active
    // until both fulfilment dimensions settle; otherwise the readiness gate
    // sees a false idle transition and the overlay disappears too early.
    let streaming = status.wanted > 0 && (status.resident < status.wanted || status.pending > 0);
    let completed = mirror.streaming
        && status.wanted > 0
        && status.resident >= status.wanted
        && status.pending == 0;
    if streaming {
        let fully_selected = status.resident >= status.wanted;
        let message = if fully_selected {
            format!(
                "Preparing terrain visuals {}/{} ({} pending)",
                status.resident, status.wanted, status.pending
            )
        } else {
            format!(
                "Streaming terrain tiles {}/{}",
                status.resident, status.wanted
            )
        };
        // Do not show a completed progress bar while render readiness is still
        // pending. Once all selected tiles are resident, the remaining work is
        // not another selected tile count, so an indeterminate indicator is the
        // truthful presentation until `pending == 0`.
        let (done, total) = if fully_selected {
            (0, 0)
        } else {
            (status.resident as u64, status.wanted as u64)
        };
        bus.set_progress(STREAM_SOURCE, message, done, total);
    } else {
        bus.remove_progress(STREAM_SOURCE);
    }
    if completed {
        bus.push(
            STREAM_SOURCE,
            lunco_status_core::status_bus::StatusLevel::Info,
            format!(
                "Terrain streaming ready ({}/{})",
                status.resident, status.wanted
            ),
        );
    }
    mirror.streaming = streaming;

    if derived.active && !mirror.deriving {
        bus.push(
            DERIVED_SOURCE,
            lunco_status_core::status_bus::StatusLevel::Info,
            format!(
                "Terrain visual preparation started ({}/{})",
                derived.ready, derived.total
            ),
        );
    }
    if derived.active {
        bus.set_progress(
            DERIVED_SOURCE,
            format!(
                "Preparing terrain visuals {}/{}",
                derived.ready, derived.total
            ),
            derived.ready as u64,
            derived.total as u64,
        );
    } else {
        bus.remove_progress(DERIVED_SOURCE);
    }
    mirror.deriving = derived.active;
}

#[cfg(test)]
mod terrain_status_tests {
    use super::*;

    #[test]
    fn terrain_progress_ends_with_current_terminal_status() {
        let mut app = App::new();
        app.insert_resource(lunco_terrain_surface::TerrainStreamStatus {
            wanted: 2,
            resident: 0,
            pending: 2,
            ..Default::default()
        })
        .insert_resource(lunco_terrain_surface::TerrainDerivedStatus::default())
        .insert_resource(lunco_status_core::status_bus::StatusBus::default())
        .insert_resource(TerrainStatusMirrorState::default())
        .add_systems(Update, report_terrain_stream_status);

        app.update();
        {
            let bus = app
                .world()
                .resource::<lunco_status_core::status_bus::StatusBus>();
            let progress = bus
                .active_progress()
                .find(|event| event.source == lunco_status_core::status_bus::TERRAIN_SOURCE)
                .expect("terrain streaming must expose live progress");
            assert_eq!(progress.message, "Streaming terrain tiles 0/2");
            assert!(bus.history().next().is_none());
        }

        *app.world_mut()
            .resource_mut::<lunco_terrain_surface::TerrainStreamStatus>() =
            lunco_terrain_surface::TerrainStreamStatus {
                wanted: 2,
                resident: 1,
                pending: 1,
                ..Default::default()
            };
        app.update();
        {
            let bus = app
                .world()
                .resource::<lunco_status_core::status_bus::StatusBus>();
            let progress: Vec<_> = bus
                .active_progress()
                .filter(|event| event.source == lunco_status_core::status_bus::TERRAIN_SOURCE)
                .collect();
            assert_eq!(
                progress.len(),
                1,
                "stream ticks must replace one live status"
            );
            assert_eq!(progress[0].message, "Streaming terrain tiles 1/2");
            assert!(bus.history().next().is_none());
        }

        *app.world_mut()
            .resource_mut::<lunco_terrain_surface::TerrainStreamStatus>() =
            lunco_terrain_surface::TerrainStreamStatus {
                wanted: 2,
                resident: 2,
                pending: 1,
                ..Default::default()
            };
        app.update();
        {
            let bus = app
                .world()
                .resource::<lunco_status_core::status_bus::StatusBus>();
            let progress = bus
                .active_progress()
                .find(|event| event.source == lunco_status_core::status_bus::TERRAIN_SOURCE)
                .expect("render-pending terrain must keep live progress");
            assert_eq!(
                progress.message,
                "Preparing terrain visuals 2/2 (1 pending)"
            );
            assert_eq!(progress.progress, Some((0, 0)));
            assert!(bus.history().next().is_none());

            *app.world_mut()
                .resource_mut::<lunco_terrain_surface::TerrainStreamStatus>() =
                lunco_terrain_surface::TerrainStreamStatus {
                    wanted: 2,
                    resident: 2,
                    ..Default::default()
                };
        }
        app.update();
        {
            let bus = app
                .world()
                .resource::<lunco_status_core::status_bus::StatusBus>();
            assert!(bus
                .active_progress()
                .all(|event| event.source != lunco_status_core::status_bus::TERRAIN_SOURCE));
            let history: Vec<_> = bus.history().collect();
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].message, "Terrain streaming ready (2/2)");
        }

        app.update();
        assert_eq!(
            app.world()
                .resource::<lunco_status_core::status_bus::StatusBus>()
                .history()
                .count(),
            1,
            "a settled terrain must not publish duplicate terminal events"
        );
    }
}

#[derive(Resource, Default)]
struct TerrainStatusMirrorState {
    streaming: bool,
    deriving: bool,
    generation: Option<(String, lunco_terrain_surface::TerrainGenPhase)>,
}

fn reset_terrain_status_mirror_on_scene_teardown(
    mut mirror: ResMut<TerrainStatusMirrorState>,
    bus: Option<ResMut<lunco_status_core::status_bus::StatusBus>>,
) {
    *mirror = TerrainStatusMirrorState::default();
    let Some(mut bus) = bus else { return };
    bus.remove_progress(lunco_status_core::status_bus::TERRAIN_SOURCE);
    bus.remove_progress(lunco_status_core::status_bus::TERRAIN_DERIVED_SOURCE);
}

/// Mirror the DEM build lifecycle into the workbench status bus. Dataset
/// provisioning is a separate, user-controlled lifecycle; once the declared
/// source is available, this entry reports the actual local generation phase
/// rather than leaving the old indeterminate preparation card as the only
/// feedback.
fn report_terrain_generation_status(
    status: Res<lunco_terrain_surface::TerrainGenStatus>,
    terrains: Query<(), With<lunco_terrain_surface::DemHeightField>>,
    faults: Option<Res<lunco_core::RuntimeFaults>>,
    bus: Option<ResMut<lunco_status_core::status_bus::StatusBus>>,
    mut mirror: ResMut<TerrainStatusMirrorState>,
) {
    let Some(mut bus) = bus else { return };
    const SOURCE: &str = lunco_status_core::status_bus::TERRAIN_BUILD_SOURCE;
    if !status.active {
        bus.remove_progress(SOURCE);
        if mirror.generation.take().is_some()
            && !faults.as_deref().is_some_and(|fault| {
                fault
                    .first
                    .as_ref()
                    .is_some_and(|f| f.kind == lunco_terrain_surface::TERRAIN_BUILD_FAULT_KIND)
            })
            && !terrains.is_empty()
        {
            bus.push(
                SOURCE,
                lunco_status_core::status_bus::StatusLevel::Info,
                "Terrain ground ready",
            );
        }
        return;
    }

    let site = if status.site.is_empty() {
        String::new()
    } else {
        format!(" — {}", status.site)
    };
    let (done, total) = status
        .fraction
        .filter(|fraction| fraction.is_finite())
        .map(|fraction| {
            let total = 1_000_u64;
            (
                ((fraction.clamp(0.0, 1.0) * total as f32).round()) as u64,
                total,
            )
        })
        .unwrap_or((0, 0));
    let key = (status.site.clone(), status.phase);
    if mirror.generation.as_ref() != Some(&key) {
        let site = if status.site.is_empty() {
            String::new()
        } else {
            format!(" — {}", status.site)
        };
        bus.push(
            SOURCE,
            lunco_status_core::status_bus::StatusLevel::Info,
            format!("{}{}", status.phase.label(), site),
        );
        mirror.generation = Some(key);
    }
    bus.set_progress(
        SOURCE,
        format!("{}{}", status.phase.label(), site),
        done,
        total,
    );
}

/// Mirror USD scene-spawn progress into the workbench
/// [`StatusBus`](lunco_status_core::status_bus::StatusBus) under
/// [`SCENE_SOURCE`](lunco_status_core::status_bus::SCENE_SOURCE), the twin of
/// [`report_terrain_stream_status`].
///
/// Two signals, because they cover different windows and neither subsumes the
/// other:
///
/// * [`SceneLoadInFlight`](lunco_usd_sim::cosim::SceneLoadInFlight) — present from
///   `LoadScene` until every visual projection phase for that stage has drained.
///   This covers the gap BEFORE any prim entity exists, which an entity count
///   alone reads as "nothing to wait for".
/// * `UsdSceneAwaitingStage` entities — prims queued on a stage that has not resolved.
///   This covers spawns with no `LoadScene` guard behind them (deferred instance
///   and reference spawns), which the resource alone would miss.
/// * `UsdSceneGeometryPending` entities — structural projection is complete, but
///   CPU-generated geometry is still being committed from the async mesh pool.
///   This is a separate visual-streaming phase, not a second scene load.
///
/// Consumed by the offline recorder's readiness gate as well as the status bar;
/// see the registration site for why the mirror lives here rather than in
/// `lunco-workbench`.
fn report_scene_spawn_status(
    in_flight: Option<Res<lunco_usd_sim::cosim::SceneLoadInFlight>>,
    awaiting: Query<(), With<lunco_usd_bevy_scene::UsdSceneAwaitingStage>>,
    projecting: Query<(), With<lunco_usd_bevy_scene::UsdSceneProjectionQueued>>,
    pending_meshes: Query<(), With<lunco_usd_bevy_scene::UsdSceneGeometryPending>>,
    coordinator: Res<lunco_core::SceneTransitionCoordinator>,
    // `Option` for the same reason as the terrain mirror: `--no-ui` is a RUNTIME
    // choice on a binary that still has the `ui` feature compiled in.
    bus: Option<ResMut<lunco_status_core::status_bus::StatusBus>>,
) {
    let Some(mut bus) = bus else { return };
    const SOURCE: &str = lunco_status_core::status_bus::SCENE_SOURCE;
    let pending = awaiting.iter().count();
    let projecting = projecting.iter().count();
    let pending_meshes = pending_meshes.iter().count();
    if matches!(
        coordinator.active(),
        Some(lunco_core::SceneTransition::Clear)
    ) {
        bus.set_progress(SOURCE, "unloading current scene", 0, 0);
        return;
    }
    if let Some(g) = in_flight {
        let message = if projecting > 0 {
            format!("projecting scene {} ({projecting} prims queued)", g.path)
        } else if pending_meshes > 0 {
            format!(
                "loading scene {} (streaming {pending_meshes} meshes)",
                g.path
            )
        } else {
            format!("loading scene {}", g.path)
        };
        // `total = 0` is the bus's "indeterminate" encoding — the number of
        // descendants can grow as each projected prim exposes its children.
        bus.set_progress(SOURCE, message, 0, 0);
    } else if pending > 0 {
        bus.set_progress(
            SOURCE,
            format!("projecting scene ({projecting} queued, {pending} pending)"),
            0,
            0,
        );
    } else if pending_meshes > 0 {
        bus.set_progress(
            SOURCE,
            format!("streaming scene visuals ({pending_meshes} meshes pending)"),
            0,
            0,
        );
    } else {
        bus.remove_progress(SOURCE);
    }
}

/// Mirror the asynchronous textured-DomeLight projection into the workbench
/// status bus. A missing source image remains pending here and therefore causes
/// the recorder to hit its loud readiness timeout instead of accepting a black
/// environment as a valid render.
fn report_dome_environment_status(
    domes: Query<(
        &lunco_usd_bevy::dome::UsdDomeEnvironment,
        Option<&lunco_usd_bevy::dome::DomeCubemap>,
        Option<&lunco_usd_bevy::dome::DomeProjection>,
    )>,
    bus: Option<ResMut<lunco_status_core::status_bus::StatusBus>>,
) {
    let Some(mut bus) = bus else { return };
    const SOURCE: &str = lunco_status_core::status_bus::DOME_SOURCE;
    let pending = domes.iter().any(|(_, cubemap, projection)| {
        projection.is_some()
            || cubemap.is_none()
            || cubemap.is_some_and(|cubemap| cubemap.0 == Handle::default())
    });
    if pending {
        bus.set_progress(SOURCE, "projecting textured DomeLight", 0, 0);
    } else {
        bus.remove_progress(SOURCE);
    }
}

/// Mirror the aggregate USD-driven Modelica lifecycle into the same status
/// channel consumed by offline recording readiness. One scene can create many
/// participants; publishing one permanent event per participant left the last
/// source filename looking like ongoing work after the scene had settled.
fn report_modelica_status(
    pending_sources: Query<(), With<lunco_usd_sim::cosim::PendingModelicaSource>>,
    models: Query<&lunco_modelica_core::ModelicaModel, With<lunco_usd_sim::cosim::UsdSourcedCosim>>,
    bus: Option<ResMut<lunco_status_core::status_bus::StatusBus>>,
    mut mirror: ResMut<ModelicaStatusMirrorState>,
) {
    let Some(mut bus) = bus else { return };
    const SOURCE: &str = lunco_status_core::status_bus::MODELICA_SOURCE;

    let pending = pending_sources.iter().count();
    // A successfully compiled model whose initial algebraic snapshot has not
    // received its first solver tick is deliberately NOT included here.
    // Offline recording freezes the simulation while it waits for this visual
    // status, so treating that state as source compilation deadlocks the gate:
    // the first tick that would make the participant Running can never happen.
    // The authoritative source lifecycle is the Modelica model itself; the
    // solver's first-step hold remains owned by the readiness subsystem.
    let mut model_count = 0;
    let mut compiling = 0;
    let mut failed = 0;
    for model in &models {
        model_count += 1;
        let ready = model.is_compiled && !model.is_compiling && model.last_error.is_none();
        if !ready && model.last_error.is_none() {
            compiling += 1;
        } else if model.last_error.is_some() {
            failed += 1;
        }
    }
    let active = pending > 0 || compiling > 0;

    if pending > 0 {
        bus.set_progress(
            SOURCE,
            format!("loading {pending} Modelica source(s)"),
            0,
            0,
        );
    } else if compiling > 0 {
        bus.set_progress(
            SOURCE,
            format!("compiling {compiling} Modelica participant(s)"),
            0,
            0,
        );
    } else {
        bus.remove_progress(SOURCE);
    }

    if !active && failed == 0 && model_count > 0 && (mirror.was_active || mirror.model_count == 0) {
        bus.push(
            SOURCE,
            lunco_status_core::status_bus::StatusLevel::Info,
            format!("Modelica ready — {model_count} participant(s)"),
        );
    }
    mirror.was_active = active;
    mirror.model_count = model_count;
}

#[derive(Resource, Default)]
struct ModelicaStatusMirrorState {
    was_active: bool,
    model_count: usize,
}

fn reset_modelica_status_mirror_on_scene_teardown(
    mut mirror: ResMut<ModelicaStatusMirrorState>,
    bus: Option<ResMut<lunco_status_core::status_bus::StatusBus>>,
) {
    *mirror = ModelicaStatusMirrorState::default();
    if let Some(mut bus) = bus {
        bus.remove_progress(lunco_status_core::status_bus::MODELICA_SOURCE);
    }
}

#[cfg(test)]
mod modelica_status_tests {
    use super::*;

    fn ready_model(name: &str) -> lunco_modelica_core::ModelicaModel {
        let mut model = lunco_modelica_core::ModelicaModel::default();
        model.model_name = name.to_owned();
        model.is_compiled = true;
        model
    }

    #[test]
    fn modelica_readiness_is_one_stable_lifecycle_event() {
        let mut app = App::new();
        app.insert_resource(lunco_status_core::status_bus::StatusBus::default())
            .init_resource::<ModelicaStatusMirrorState>()
            .add_systems(Update, report_modelica_status);
        let first = app
            .world_mut()
            .spawn((lunco_usd_sim::cosim::UsdSourcedCosim, ready_model("First")))
            .id();
        app.world_mut()
            .spawn((lunco_usd_sim::cosim::UsdSourcedCosim, ready_model("Second")));

        app.update();
        app.update();
        let bus = app
            .world()
            .resource::<lunco_status_core::status_bus::StatusBus>();
        assert_eq!(bus.history().count(), 1);
        assert_eq!(
            bus.history().next().map(|event| event.message.as_str()),
            Some("Modelica ready — 2 participant(s)")
        );

        app.world_mut()
            .entity_mut(first)
            .get_mut::<lunco_modelica_core::ModelicaModel>()
            .expect("Modelica model")
            .is_compiling = true;
        app.update();
        assert!(app
            .world()
            .resource::<lunco_status_core::status_bus::StatusBus>()
            .active_progress()
            .any(|event| event.source == lunco_status_core::status_bus::MODELICA_SOURCE));

        app.world_mut()
            .entity_mut(first)
            .get_mut::<lunco_modelica_core::ModelicaModel>()
            .expect("Modelica model")
            .is_compiling = false;
        app.update();
        let bus = app
            .world()
            .resource::<lunco_status_core::status_bus::StatusBus>();
        // Returning to the same ready snapshot after a compile transition
        // must remain one stable lifecycle event. StatusBus coalesces
        // consecutive identical discrete snapshots by contract.
        assert_eq!(bus.history().count(), 1);
        assert_eq!(bus.history_total(), 1);
        assert!(bus
            .active_progress()
            .all(|event| { event.source != lunco_status_core::status_bus::MODELICA_SOURCE }));
    }
}
