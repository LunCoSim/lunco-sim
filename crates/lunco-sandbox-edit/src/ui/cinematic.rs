//! Cinematic camera authoring: capture the current view as a USD `def Camera`.
//!
//! The lowest-friction authoring gesture there is (Blender's
//! Ctrl+Alt+Numpad0, Unreal's "Create Camera Here"): fly the free camera until
//! the framing is right, then press the button — the pose you are looking
//! through becomes a real prim in the scene. Doc 50 §10.
//!
//! **A command, not a click observer.** The capture needs the grid hierarchy
//! (`ChildOf`/`Grid`/`CellCoord`) to resolve a grid-absolute pose, which a
//! `Panel` cannot query — it only reaches resources and single components. So
//! the panel triggers [`AddCameraHere`] and an observer does the work, which
//! also makes the verb reachable from rhai / the HTTP API / MCP for free rather
//! than trapping it behind a button.
//!
//! Authoring goes through `ApplyUsdOp`, never a direct ECS spawn: that is what
//! puts the new camera in the journal, the undo stack, the save file and the
//! network — an edit that bypasses it escapes all four.
//!
//! The panel also carries the **transport** (restart / play / loop / scrub) and
//! toggles the **trajectory overlay**, so authoring a shot and replaying it are
//! one docked surface in Build mode rather than a floating pill.

use bevy::camera::RenderTarget;
use bevy::math::{DQuat, EulerRot};
use bevy::prelude::*;
use bevy_egui::egui;
use big_space::prelude::{CellCoord, Grid};
use lunco_core::{on_command, register_commands, Command};
use lunco_render::SceneCamera;
use lunco_time::{AnimationPreview, ControlAnimation, Playback, TransportMode};
use lunco_usd::commands::ApplyUsdOp;
use lunco_usd::document::{LayerId, UsdOp};
use lunco_usd::registry::UsdDocumentRegistry;
use lunco_usd_bevy::camera_path::{eval_curve, AimMode, CameraPath};
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

/// How many points to sample along a camera path when drawing it. The authored
/// keys are the shot; this is just how finely the polyline approximates the
/// interpolation between them.
const PATH_SAMPLES: usize = 96;

/// The camera path the panel's transport drives.
///
/// A path owns a per-object driven clock, so the shared `AnimationPreview`
/// transport does not reach it — without this the panel's ⏮/▶ would silently do
/// nothing to a path-driven shot. A `Panel` cannot run queries, so a system
/// publishes the domain here for it to read.
#[derive(Resource, Default)]
pub struct CinematicTarget {
    /// The active path's driven domain; `None` ⇒ the panel falls back to the
    /// shared animation preview.
    pub domain: Option<Entity>,
}

/// Publish the first camera path's domain for the panel to drive.
///
/// One path today. With several, this becomes "the selected path" — the panel is
/// already written against whatever this resource points at.
pub fn track_active_camera_path(
    q_paths: Query<&CameraPath>,
    mut target: ResMut<CinematicTarget>,
) {
    let first = q_paths.iter().next().map(|p| p.domain);
    if target.domain != first {
        target.domain = first;
    }
}

/// Trajectory-overlay state (UI-local, like `TerrainToolState`).
#[derive(Resource)]
pub struct CinematicViz {
    /// Draw every animated camera's path in the viewport.
    pub show_paths: bool,
}

impl Default for CinematicViz {
    fn default() -> Self {
        // On by default: an authored camera move you cannot see is the exact
        // thing that made the frozen-camera bug (doc 50 §8a) invisible.
        Self { show_paths: true }
    }
}

/// Draw each [`CameraPath`]'s curve, its control points, and a live dot at the
/// playhead.
///
/// Evaluates the curve with the driver's OWN `eval_curve`, so what you see is
/// exactly where the camera will fly — the overlay cannot drift from the motion,
/// because it is the same function. It reads authored control points, not a trace
/// of past positions, so the path is visible before playback ever runs and
/// updates the instant a point moves.
///
/// A stop-gap: `BasisCurves` is real geometry and should render itself (a prim in
/// the scene AND in usdview). Nothing instantiates curve geometry yet, so without
/// this the path is invisible. When curve rendering lands, this overlay's job
/// shrinks to the editing affordances (handles + playhead).
///
/// **Frame**: control points are the curve prim's own geometry, so they are in its
/// local space; the curve entity's `GlobalTransform` maps them to render space,
/// which is where gizmos draw. Same composition Bevy gives the camera — no grid
/// math, no render-vs-grid frame bug by construction.
pub fn draw_camera_paths(
    viz: Res<CinematicViz>,
    resolved: Res<lunco_time::ResolvedDomains>,
    q_paths: Query<(&CameraPath, &GlobalTransform)>,
    q_playback: Query<&Playback>,
    q_gt: Query<&GlobalTransform>,
    q_active: Query<(Entity, &Camera), With<Camera3d>>,
    mut gizmos: Gizmos,
) {
    if !viz.show_paths {
        return;
    }
    // The camera you are looking through, if any.
    let looking_through = q_active
        .iter()
        .find(|(_, c)| c.is_active)
        .map(|(e, _)| e);

    for (path, gt) in q_paths.iter() {
        // Don't draw a path you are flying. Every marker would land on the eye:
        // the playhead sphere smears across the near plane (the stray yellow
        // flashes), and the curve itself — which passes exactly through the camera
        // — projects as a line off both edges of frame. A path is only legible
        // from a DIFFERENT viewpoint, so this is the honest gate rather than a
        // depth tweak.
        if looking_through == Some(path.camera) {
            continue;
        }
        let at = |u: f32| gt.transform_point(eval_curve(&path.points, path.basis, path.periodic, u));

        let pts: Vec<Vec3> = (0..=PATH_SAMPLES)
            .map(|i| at(i as f32 / PATH_SAMPLES as f32))
            .collect();
        gizmos.linestrip(pts.iter().copied(), Color::srgb(0.25, 0.8, 1.0));

        // The control points — what you drag — each with the view direction the
        // camera will have there. Aim is a track, so a point inside a "look at the
        // lander" stretch and one inside a "free look" stretch genuinely differ;
        // reading `aim_at` (rather than assuming one target) is what makes the
        // arrows tell the truth.
        let n = path.points.len().max(1);
        for (i, p) in path.points.iter().enumerate() {
            let at_pt = gt.transform_point(*p);
            gizmos.sphere(
                Isometry3d::from_translation(at_pt),
                0.8,
                Color::srgb(1.0, 0.6, 0.1),
            );
            // Time at this point, on the path's own clock.
            let u_i = i as f32 / n as f32;
            let t_i = span_of(&q_playback, path).map(|(s, e)| s + (e - s) * u_i as f64);
            let dir = match (t_i.map(|t| path.aim_at(t)), path.aim_at(0.0)) {
                (Some(AimMode::Target(e)), _) | (None, AimMode::Target(e)) => q_gt
                    .get(e)
                    .ok()
                    .map(|tgt| (tgt.translation() - at_pt).normalize_or_zero()),
                (Some(AimMode::Manual), _) => None, // user steers here — nothing to draw
                _ => {
                    let ahead = gt.transform_point(eval_curve(
                        &path.points,
                        path.basis,
                        path.periodic,
                        (u_i + 0.01).min(1.0),
                    ));
                    Some((ahead - at_pt).normalize_or_zero())
                }
            };
            if let Some(d) = dir.filter(|d| d.length_squared() > 1e-6) {
                gizmos.arrow(at_pt, at_pt + d * 6.0, Color::srgb(0.4, 1.0, 0.6));
            }
        }

        // Where the playhead sits on the curve right now.
        if let (Ok(pb), Some(t)) = (q_playback.get(path.domain), resolved.get(path.domain)) {
            let span = (pb.end - pb.start).max(f64::EPSILON);
            let u = (((t - pb.start) / span) as f32).clamp(0.0, 1.0);
            gizmos.sphere(
                Isometry3d::from_translation(at(u)),
                1.0,
                Color::srgb(1.0, 0.9, 0.2),
            );
        }
    }
}

/// The path clock's `[start, end]`, if it has one.
fn span_of(q_playback: &Query<&Playback>, path: &CameraPath) -> Option<(f64, f64)> {
    q_playback.get(path.domain).ok().map(|pb| (pb.start, pb.end))
}

/// Capture the active viewport camera's pose as a new `def Camera` prim.
///
/// Authored into [`LayerId::root`] — the authored scene, serialized on Save. A
/// captured shot is a durable edit to the twin, unlike the gizmo/waypoint
/// interactions that write the ephemeral `runtime` overlay and vanish.
#[Command(default)]
pub struct AddCameraHere {
    /// Prim name for the new camera. `None` picks the first free `View_N`.
    pub name: Option<String>,
}

#[on_command(AddCameraHere)]
fn on_add_camera_here(
    trigger: On<AddCameraHere>,
    q_cam: Query<(Entity, &Camera, &RenderTarget), With<Camera3d>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    usd_registry: Res<UsdDocumentRegistry>,
    mut commands: Commands,
) {
    // The camera the user is actually looking through. `is_active` alone is not
    // enough — a render-to-texture preview camera is active too, so require a
    // window target (same rule the gizmo picks its camera by).
    let Some((cam_entity, _, _)) = q_cam
        .iter()
        .find(|(_, cam, target)| cam.is_active && matches!(target, RenderTarget::Window(_)))
    else {
        warn!("[cinematic] no active window camera to capture");
        return;
    };

    // Grid-ABSOLUTE pose. `world_pose` walks the grid hierarchy and returns the
    // authored frame directly — do NOT read the camera's `GlobalTransform`,
    // which is the render/floating-origin frame and would author a pose that
    // silently drifts with the origin (doc 50 §5; the repo's classic frame bug).
    let Some((pos, rot)) = lunco_core::coords::world_pose(cam_entity, &q_parents, &q_grids, &q_spatial)
    else {
        warn!("[cinematic] camera {cam_entity:?} has no resolvable grid pose");
        return;
    };

    let Some(doc) = workspace
        .and_then(|w| w.0.active_document)
        .or_else(|| usd_registry.ids().next())
    else {
        warn!("[cinematic] no active USD document to author into");
        return;
    };
    let Some(host) = usd_registry.host(doc) else {
        warn!("[cinematic] no USD host for document {doc:?}");
        return;
    };

    let root = lunco_usd_bevy::layer_default_prim(host.document().data())
        .map(|p| format!("/{p}"))
        .unwrap_or_else(|| "/".to_string());

    // `AddPrim` on an existing prim is a rejection, not a merge — so find a free
    // name rather than letting the op fail silently on the second capture.
    let name = match &trigger.event().name {
        Some(n) => n.clone(),
        None => {
            let mut n = 1;
            loop {
                let candidate = format!("View_{n}");
                if !prim_exists(host, &join(&root, &candidate)) {
                    break candidate;
                }
                n += 1;
                if n > 999 {
                    warn!("[cinematic] giving up naming a camera after 999 tries");
                    return;
                }
            }
        }
    };
    let path = join(&root, &name);
    if prim_exists(host, &path) {
        warn!("[cinematic] prim already exists at {path} — not overwriting");
        return;
    }

    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: root,
            name: name.clone(),
            type_name: Some("Camera".to_string()),
            reference: None,
        },
    });
    // `SetTranslate`/`SetRotate` synthesize `xformOpOrder` when the prim has
    // none, which a just-created Camera never does — so the op stack comes out
    // right without hand-authoring the token array.
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetTranslate {
            edit_target: LayerId::root(),
            path: path.clone(),
            value: [pos.x, pos.y, pos.z],
        },
    });
    let (rx, ry, rz) = euler_degrees(rot);
    commands.trigger(ApplyUsdOp {
        doc,
        op: UsdOp::SetRotate {
            edit_target: LayerId::root(),
            path: path.clone(),
            value: [rx, ry, rz],
        },
    });

    info!("[cinematic] captured {path} at {pos:?}");
}

register_commands!(on_add_camera_here);

/// `xformOp:rotateXYZ` is Euler XYZ in **degrees** (matching `UsdOp::SetRotate`).
fn euler_degrees(rot: DQuat) -> (f64, f64, f64) {
    let (x, y, z) = rot.to_euler(EulerRot::XYZ);
    (x.to_degrees(), y.to_degrees(), z.to_degrees())
}

fn join(root: &str, name: &str) -> String {
    if root == "/" {
        format!("/{name}")
    } else {
        format!("{root}/{name}")
    }
}

fn prim_exists(
    host: &lunco_doc::DocumentHost<lunco_usd::document::UsdDocument>,
    path: &str,
) -> bool {
    let Ok(sdf) = lunco_usd_bevy::SdfPath::new(path) else {
        return false;
    };
    host.document().data().spec(&sdf).is_some()
        || host.document().runtime_data().spec(&sdf).is_some()
}

/// Cinematic palette — capture views, list the scene's cameras.
pub struct CinematicPanel;

impl Panel for CinematicPanel {
    fn id(&self) -> PanelId {
        PanelId("cinematic_tools")
    }
    fn title(&self) -> String {
        "🎬 Cinematic".into()
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }
    fn transparent_background(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let Some(mantle) = ctx
            .resource::<lunco_theme::Theme>()
            .map(|theme| theme.colors.mantle)
        else {
            return;
        };
        egui::Frame::new()
            .fill(mantle)
            .inner_margin(8.0)
            .corner_radius(4)
            .show(ui, |ui| {
                ui.heading("Cinematic");
                ui.add_space(4.0);

                if ui
                    .button("📷 Add Camera Here")
                    .on_hover_text("Capture the current view as a new Camera prim in the scene")
                    .clicked()
                {
                    ctx.trigger(AddCameraHere { name: None });
                }
                ui.small("Fly the view, then capture — the camera is authored into");
                ui.small("the scene and saved with it.");

                ui.add_space(6.0);
                let show = ctx.resource::<CinematicViz>().is_some_and(|v| v.show_paths);
                let mut show_mut = show;
                if ui
                    .checkbox(&mut show_mut, "Show camera paths")
                    .on_hover_text("Draw each animated camera's authored trajectory in the viewport")
                    .changed()
                {
                    ctx.defer(move |world| {
                        if let Some(mut v) = world.get_resource_mut::<CinematicViz>() {
                            v.show_paths = show_mut;
                        }
                    });
                }

                ui.separator();
                transport_section(ui, ctx);
            });
    }
}

/// Transport for the animation preview — restart / play / loop / scrub.
///
/// Every control dispatches the one [`ControlAnimation`] verb, so this panel,
/// the inspector's transport, rhai and the HTTP/MCP API all drive a single code
/// path.
fn transport_section(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    // Drive the active camera path's OWN clock when there is one — it is a
    // per-object driven domain, so the shared preview transport does not reach it.
    let path_domain = ctx.resource::<CinematicTarget>().and_then(|t| t.domain);
    let Some(domain) = path_domain.or_else(|| ctx.resource::<AnimationPreview>().map(|p| p.domain))
    else {
        ui.small("No animation preview in this scene.");
        return;
    };
    // `None` targets the preview; an explicit path domain targets that path.
    let target = path_domain;
    let Some(pb) = ctx.get::<Playback>(domain) else {
        ui.small("No playback on the preview domain.");
        return;
    };
    let (start, end, head, rate, looping) = (pb.start, pb.end, pb.head, pb.rate, pb.looping);
    let playing = matches!(pb.mode, TransportMode::Playing);
    let bounded = end > start;

    ui.horizontal(|ui| {
        // Restart = seek-to-start AND play, in one verb. Seeks to `start`, not a
        // literal 0.0: `step_playhead` clamps to [start, end], so on a clip that
        // starts late a hardcoded 0 lands outside the range and snaps forward on
        // the next step.
        if ui
            .button("⏮")
            .on_hover_text("Restart the camera move from the beginning")
            .clicked()
        {
            ctx.trigger(ControlAnimation {
                target,
                playing: Some(true),
                seek_secs: Some(start),
                ..default()
            });
        }
        let (icon, hint) = if playing { ("⏸", "Pause") } else { ("▶", "Play") };
        if ui.button(icon).on_hover_text(hint).clicked() {
            ctx.trigger(ControlAnimation {
                target,
                playing: Some(!playing),
                ..default()
            });
        }
        let mut loop_mut = looping;
        if ui
            .add_enabled(bounded, egui::Checkbox::new(&mut loop_mut, "🔁"))
            .on_hover_text("Loop at the end instead of stopping")
            .on_disabled_hover_text("Needs a bounded clip range — nothing is keyed yet")
            .changed()
        {
            ctx.trigger(ControlAnimation {
                target,
                looping: Some(loop_mut),
                ..default()
            });
        }
    });

    if !bounded {
        ui.small("No keyed animation in this scene yet.");
        return;
    }

    // Scrub — the other half of the author loop (scrub → fly → capture).
    let mut t = head;
    if ui
        .add(egui::Slider::new(&mut t, start..=end).fixed_decimals(1).suffix(" s").text("Time"))
        .changed()
    {
        ctx.trigger(ControlAnimation {
            target,
            seek_secs: Some(t),
            ..default()
        });
    }
    let mut r = rate;
    if ui
        .add(egui::Slider::new(&mut r, 0.0..=4.0).fixed_decimals(2).suffix("×").text("Rate"))
        .changed()
    {
        ctx.trigger(ControlAnimation {
            target,
            rate: Some(r),
            ..default()
        });
    }
}
