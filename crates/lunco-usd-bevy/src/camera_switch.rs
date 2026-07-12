//! Viewport active-camera switching + the viewport-camera **reconciler**.
//!
//! The scene has one main-window [`Viewport`](lunco_core::SceneViewport): it
//! owns *which* camera renders (its **active camera**), *whether* it renders
//! (visibility), and *what rect* it occupies — modelled on an Omniverse
//! Viewport. [`reconcile_scene_viewport`] is the **single authority** that
//! turns that into Bevy's per-camera `Camera::is_active` + `Camera::viewport`;
//! nothing else writes those for window cameras. Contributors only supply data:
//! - the switch here rebinds the viewport's active camera;
//! - the workbench sets visibility + rect from its layout perspective.
//!
//! This lives in `lunco-usd-bevy` (avatar-free, present in every windowed
//! binary) so switching works in a static/headless world with no avatar and no
//! input. A *switchable* camera is any [`Camera3d`] with a window
//! [`RenderTarget`]: every USD `def Camera`, plus whatever free/avatar camera a
//! host adds. RTT (`Image`-target) cameras and the egui `Camera2d` are never
//! touched.
//!
//! Switch surfaces, one mechanism — all funnel through [`ActivateCamera`] →
//! rebind [`SceneViewport::active_camera`](lunco_core::SceneViewport):
//! - [`SetActiveCamera`] — command (API + rhai `set_camera("Name")`);
//! - the `KeyC` hotkey ([`cycle_active_camera`]) when a host runs with input.

use bevy::camera::{RenderTarget, Viewport};
use bevy::prelude::*;
use big_space::prelude::{FloatingOrigin, Grid};
use lunco_core::{on_command, Command, LocalAvatar, SceneViewport};

/// Switch the viewport's active camera to the `Camera3d` whose `Name` matches.
///
/// Works with no avatar present. `name` matches the full USD prim path *or*
/// its leaf, so a cutscene can `set_camera("ChaseCam")` to reach
/// `/World/Rover/ChaseCam`, or `set_camera("WideShot")` for a scene camera.
#[Command(default)]
pub struct SetActiveCamera {
    /// Camera name (full USD prim path or its leaf).
    pub name: String,
}

/// Internal trigger: bind `.0` as the viewport's active camera. Both the
/// name-based command and the cycle hotkey resolve to an entity and fire this,
/// so the binding is written in exactly one observer.
#[derive(Event)]
pub struct ActivateCamera(pub Entity);

/// Command handler: resolve `SetActiveCamera.name` → a camera entity and fire
/// [`ActivateCamera`]. Matches the full USD prim path *or* its leaf.
#[on_command(SetActiveCamera)]
pub fn on_set_active_camera(
    trigger: On<SetActiveCamera>,
    q_cams: Query<(Entity, &Name), With<Camera3d>>,
    mut commands: Commands,
) {
    let want = cmd.name.trim();
    let hit = q_cams.iter().find(|(_, n)| {
        let s = n.as_str();
        s == want || s.rsplit('/').next() == Some(want)
    });
    match hit {
        Some((e, _)) => {
            commands.trigger(ActivateCamera(e));
        }
        None => warn!("[camera] SetActiveCamera: no camera named '{want}'"),
    }
}

/// `KeyC`: advance the viewport's active camera to the next window camera
/// (stable order by `Name`, wrapping). No-op with fewer than two window
/// cameras or no input. "Current" is the viewport binding, not raw `is_active`
/// (which the visibility gate may have cleared).
pub fn cycle_active_camera(
    // Optional: a static/headless world has no `ButtonInput` resource (no input
    // plugin). It simply never cycles — the command path still works there.
    keys: Option<Res<ButtonInput<KeyCode>>>,
    vp: Res<SceneViewport>,
    q_cams: Query<(Entity, &RenderTarget, &Name), With<Camera3d>>,
    mut commands: Commands,
) {
    let Some(keys) = keys else {
        return;
    };
    if !keys.just_pressed(KeyCode::KeyC) {
        return;
    }
    // Don't hijack modified chords (Ctrl+C copy, etc.).
    if keys.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::AltLeft,
        KeyCode::AltRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]) {
        return;
    }

    let mut cams: Vec<(Entity, &str)> = q_cams
        .iter()
        .filter(|(_, target, _)| matches!(target, RenderTarget::Window(_)))
        .map(|(e, _, name)| (e, name.as_str()))
        .collect();
    if cams.len() < 2 {
        return;
    }
    cams.sort_by(|a, b| a.1.cmp(b.1));
    let cur = vp
        .active_camera
        .and_then(|a| cams.iter().position(|(e, _)| *e == a))
        .unwrap_or(0);
    let next = cams[(cur + 1) % cams.len()].0;
    commands.trigger(ActivateCamera(next));
}

/// Rebind the viewport's active camera. The reconciler actuates
/// `is_active`/`viewport` from this — this observer never touches cameras
/// directly (single-writer discipline).
pub fn on_activate_camera(
    trigger: On<ActivateCamera>,
    q_cams: Query<&RenderTarget, With<Camera3d>>,
    mut vp: ResMut<SceneViewport>,
) {
    let target = trigger.event().0;
    match q_cams.get(target) {
        Ok(t) if matches!(t, RenderTarget::Window(_)) => {
            vp.active_camera = Some(target);
            info!("[camera] viewport → {target:?}");
        }
        Ok(_) => warn!("[camera] activate: {target:?} does not render to a window"),
        Err(_) => warn!("[camera] activate: {target:?} is not a Camera3d"),
    }
}

/// The local-avatar camera claims the viewport when it appears.
///
/// The startup provisional camera and the USD-authored avatar takeover both add
/// `LocalAvatar`; this makes whichever one exists the **default** view — a later
/// `set_camera(...)` overrides it. It also ensures the player's eye wins over a
/// non-avatar camera (e.g. a celestial observer) that happened to spawn first,
/// which the reconciler's "keep a valid binding" rule wouldn't correct on its own.
pub fn bind_avatar_camera_on_add(
    add: On<Add, LocalAvatar>,
    q: Query<&RenderTarget, With<Camera3d>>,
    mut vp: ResMut<SceneViewport>,
) {
    let e = add.entity;
    if let Ok(RenderTarget::Window(_)) = q.get(e) {
        vp.active_camera = Some(e);
    }
}

/// The **single authority** over window-camera `is_active` + `viewport`.
///
/// Reads the [`SceneViewport`] (active-camera binding + visibility + rect) and
/// actuates it: exactly the bound camera is active (and only when visible); all
/// other window cameras are off. RTT (`Image`-target) cameras are ignored.
/// Also relocates the big_space [`FloatingOrigin`] onto the active camera when
/// it is grid-direct.
///
/// Robust by construction: the binding is revalidated every frame and falls
/// back to the local-avatar camera (else the lowest-entity window camera) when
/// it's unset or stale — so async spawns and provisional→avatar takeover never
/// leave zero or many active cameras.
pub fn reconcile_scene_viewport(
    mut vp: ResMut<SceneViewport>,
    mut q_cams: Query<(Entity, &mut Camera, &RenderTarget, Option<&ChildOf>), With<Camera3d>>,
    q_avatar_cam: Query<Entity, (With<Camera3d>, With<LocalAvatar>)>,
    q_grids: Query<(), With<Grid>>,
    q_origins: Query<Entity, With<FloatingOrigin>>,
    mut commands: Commands,
) {
    let is_window = |t: &RenderTarget| matches!(t, RenderTarget::Window(_));

    // ── Resolve the bound camera (revalidate + default) ──────────────────
    // Keep the binding if it still points at a window camera; else fall back
    // to the local-avatar camera, else the lowest-entity window camera. This
    // is what makes takeover + async spawn robust.
    let bound_valid = vp
        .active_camera
        .filter(|e| q_cams.get(*e).map_or(false, |(_, _, t, _)| is_window(t)));
    let active = bound_valid.or_else(|| {
        q_avatar_cam
            .iter()
            .find(|e| q_cams.get(*e).map_or(false, |(_, _, t, _)| is_window(t)))
            .or_else(|| {
                let mut ws: Vec<Entity> = q_cams
                    .iter()
                    .filter(|(_, _, t, _)| is_window(t))
                    .map(|(e, _, _, _)| e)
                    .collect();
                ws.sort();
                ws.into_iter().next()
            })
    });
    if vp.active_camera != active {
        vp.active_camera = active;
    }

    // Can the active camera host the FloatingOrigin? (grid-direct only)
    let grid_direct = active
        .and_then(|e| q_cams.get(e).ok())
        .and_then(|(_, _, _, parent)| parent)
        .map(|c| q_grids.contains(c.parent()))
        .unwrap_or(false);

    let visible = vp.visible;
    let rect = vp.rect;

    // ── Actuate: the ONE writer of window-camera is_active + viewport ────
    for (e, mut cam, target, _) in q_cams.iter_mut() {
        if !is_window(target) {
            continue; // RTT/offscreen cameras are self-managed
        }
        let want_active = Some(e) == active && visible;
        if cam.is_active != want_active {
            cam.is_active = want_active;
        }
        let want_vp = if Some(e) == active {
            rect.map(|(pos, size)| Viewport {
                physical_position: pos,
                physical_size: size,
                ..default()
            })
        } else {
            None
        };
        // Compare pos+size only (Viewport's `depth: Range<f32>` isn't `Eq`).
        let same = match (&cam.viewport, &want_vp) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.physical_position == b.physical_position && a.physical_size == b.physical_size
            }
            _ => false,
        };
        if !same {
            cam.viewport = want_vp;
        }
    }

    // ── FloatingOrigin follows the active camera (grid-direct only) ──────
    // Only mutate when it actually needs to move — re-inserting the marker
    // every frame churns big_space's recentring and jitters camera follow.
    if let (Some(active), true) = (active, grid_direct) {
        let active_has_origin = q_origins.contains(active);
        for prior in q_origins.iter() {
            if prior != active {
                commands.entity(prior).remove::<FloatingOrigin>();
            }
        }
        if !active_has_origin {
            commands.entity(active).insert(FloatingOrigin);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_cam(is_active: bool, name: &str) -> impl Bundle {
        (
            Camera3d::default(),
            Camera {
                is_active,
                ..default()
            },
            RenderTarget::Window(bevy::window::WindowRef::Primary),
            Name::new(name.to_string()),
        )
    }

    fn active_set(app: &mut App) -> Vec<Entity> {
        let mut q = app
            .world_mut()
            .query_filtered::<(Entity, &Camera), With<Camera3d>>();
        q.iter(app.world())
            .filter(|(_, c)| c.is_active)
            .map(|(e, _)| e)
            .collect()
    }

    /// The reconciler activates exactly the bound camera and deactivates every
    /// other window camera — even stray ones spawned active.
    #[test]
    fn reconciler_activates_only_the_bound_camera() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .add_systems(Update, reconcile_scene_viewport);
        let _a = app.world_mut().spawn(window_cam(true, "A")).id();
        let b = app.world_mut().spawn(window_cam(false, "B")).id();
        let _c = app.world_mut().spawn(window_cam(true, "C")).id(); // stray active
        app.world_mut()
            .resource_mut::<SceneViewport>()
            .active_camera = Some(b);

        app.update();

        assert_eq!(active_set(&mut app), vec![b], "only the bound camera renders");
    }

    /// When the viewport is not visible (workbench Design perspective), no
    /// window camera renders — but the binding is preserved for restore.
    #[test]
    fn invisible_viewport_deactivates_all() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .add_systems(Update, reconcile_scene_viewport);
        let b = app.world_mut().spawn(window_cam(true, "B")).id();
        {
            let mut vp = app.world_mut().resource_mut::<SceneViewport>();
            vp.active_camera = Some(b);
            vp.visible = false;
        }

        app.update();

        assert!(active_set(&mut app).is_empty(), "nothing renders while hidden");
        assert_eq!(
            app.world().resource::<SceneViewport>().active_camera,
            Some(b),
            "binding preserved across a hide"
        );
    }
}
