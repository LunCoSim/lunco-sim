//! `ViewportPanel` and the workbench's 3D-viewport plumbing.
//!
//! ## Architecture (read this if anything here looks weird)
//!
//! Two cameras share the window's Bevy main texture, with the scene camera
//! rendering the full window behind the active workbench layout:
//!
//! 1. The scene `Camera3d` (order 0), declared by the canonical
//!    [`lunco_render::SceneCamera`] intent. It renders the 3D **full-window** in
//!    both viewport-only and docked perspectives and **clears** the window's
//!    main texture each frame. The dock leaf rect is still measured separately
//!    for egui occlusion and scene-picking geometry; it is not a camera crop.
//! 2. The egui host `Camera2d` (order 1) — [`WorkbenchEguiHost`], carrying
//!    `PrimaryEguiContext`, auto-spawned by [`ensure_egui_host`]. bevy_egui
//!    paints the chrome into the same main texture after the scene pass, with
//!    `ClearColorConfig::None`, so opaque panels cover the scene and transparent
//!    regions keep the scene pixels.
//!
//! The host exists as its own camera because scene cameras are transient — USD
//! scenes spawn them, `camera_switch` swaps them, avatars despawn them — while
//! the egui context must be stable for the life of the app.
//!
//! ## The framebuffer contract (the load-bearing invariant)
//!
//! **The two cameras must share one main texture.** Bevy keys a target's main
//! textures by `(target, usages, format, msaa)`. The scene camera clears that
//! texture; egui paints on the cleared scene pixels. The host therefore follows
//! the active scene camera's MSAA and HDR format, and the sync is change-driven
//! across camera replacement, settings changes, resize, and scene teardown.
//! A private host texture with `ClearColorConfig::None` is an accumulation
//! buffer: panels removed by a perspective switch remain baked into it.
//!
//! ## Why this is robust *by design*
//!
//! - **Bevy_egui auto-pick can't race.** [`ensure_egui_host`] disables
//!   `EguiGlobalSettings::auto_create_primary_context` and pins the
//!   marker on exactly one camera. Extra Camera2d entities (vello
//!   diagram targets, USD preview tabs, …) are harmless because they
//!   target offscreen Images.
//! - **The camera intent is the ownership boundary.** `SceneCamera` is
//!   authored by USD/avatar projection code and is observed by the render
//!   binder. The Workbench consumes that same component; it does not add a
//!   presentation-only tag after the render pipeline exists.
//! - **Sentinels catch regressions.** [`check_camera_invariants`] warns when a
//!   new window-targeting `Camera3d` has no `SceneCamera` intent. Such a camera
//!   has no owner and must be fixed at its spawn/binding site, not adopted here.
//! - **The texture key is maintained by its owner.** [`sync_egui_host_msaa`]
//!   follows the active window scene camera's MSAA and HDR format rather than
//!   relying on a hardcoded presentation setting. This keeps the shared target
//!   valid when the user changes graphics quality or a Twin replaces cameras.
//!
//! ## When NO 3D camera renders the window
//!
//! Design-style perspectives and the Modelica workbench have no active window
//! `Camera3d`, so nothing clears the target. `render_layout` covers exactly
//! that case by painting a full-window backdrop on egui's background layer
//! (`needs_full_backdrop`). That is why chrome-only apps are not affected by
//! the invariant above.
//!
//! ## What goes where
//!
//! - `ensure_egui_host` (Startup) — auto-spawn the egui host.
//! - `sync_egui_host_msaa` (Update) — maintain the shared main-texture key.
//! - `apply_workbench_viewport` (PostUpdate, before `CameraUpdateSystems`)
//!   — publish scene visibility into `SceneViewport`; the camera remains
//!   full-window while the dock leaf rect stays in `PanelRects`/`ScenePickGate`.
//! - `check_camera_invariants` (Update, runs on `Added<Camera3d>`) —
//!   loud failure if a window-targeting Camera3d shows up without its camera intent.
//! - `ViewportPanel::render` — records the panel's screen rect into
//!   `PanelRects` and reserves the space; the 3D camera does the
//!   actual painting.

use bevy::prelude::*;
// `bevy::camera::*` re-exports work on *both* native and
// `--no-default-features` wasm builds. `bevy::render::camera::*` only
// exists when the `bevy_render` feature is on, which wasm strips.
use crate::{Panel, PanelCtx, PanelId, PanelScrollPolicy, PanelSlot};
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, Hdr, RenderTarget};
use bevy_egui::{egui, EguiGlobalSettings, PrimaryEguiContext};
use lunco_control_core::{IntentState, LocalIntentSurface};
use lunco_core::SceneViewport;
use lunco_input_core::InputBindingsSettings;
use lunco_render::SceneCamera;
use lunco_workbench_core::presentation::ViewportPlaceholder;
use lunco_workbench_core::scene_pick::{EguiPointerState, ScenePickGate, SceneTarget};
use lunco_workbench_core::viewport::{PanelRect, PanelRects, VIEWPORT_PANEL_ID};

/// Marker component on the egui-owning camera for one window.
///
/// Auto-inserted by [`ensure_egui_host`] alongside `Camera2d` and
/// `PrimaryEguiContext`. The host spawn is a single-site concern, so a plain
/// marker keeps the ownership boundary explicit.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct WorkbenchEguiHost;

/// Measurement emitted by the transparent viewport panel after it has read
/// its egui geometry. The panel cannot mutate either view-model directly;
/// this typed event keeps the write on the workbench side of the boundary.
#[derive(Event, Debug, Clone, Copy)]
struct ViewportPanelMeasured {
    rect: PanelRect,
    over_scene: bool,
}

/// Workbench-central panel that reserves a rect for the 3D viewport.
///
/// The panel itself paints nothing — its only job is to record its
/// screen-space rect into [`PanelRects`] each frame so the
/// [`apply_workbench_viewport`] system can drive every
/// [`SceneCamera`]-owned camera's `Camera::viewport`.
///
/// Background: transparent by design. The scene camera renders the full window
/// underneath the egui host, while the workbench's occlusion-aware picking gate
/// keeps pointer events over chrome away from the scene. This lets transparent
/// tab content show the live scene instead of exposing the swapchain clear.
pub struct ViewportPanel;

impl Panel for ViewportPanel {
    fn id(&self) -> PanelId {
        VIEWPORT_PANEL_ID
    }

    fn title(&self) -> String {
        // Empty title — there's nothing useful to show in a tab header
        // for "the 3D viewport". egui_dock still draws the bar (we
        // can't hide it per-leaf in 0.18) but the content is blank.
        String::new()
    }

    /// Never listed: it is the centre fixture (empty title, not closable), not
    /// something a user opens.
    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Hidden
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn closable(&self) -> bool {
        // If the user closes the viewport tab, the centre region
        // collapses and the side panels reflow oddly. Keep it docked.
        false
    }

    fn scene_target(&self) -> Option<lunco_workbench_core::PanelRenderTarget> {
        // This IS the full-window scene — exempt from the pick gate's chrome-card
        // recording; the 3D camera paints through this transparent leaf.
        Some(lunco_workbench_core::PanelRenderTarget::MainViewport)
    }

    fn transparent_background(&self) -> bool {
        // TRANSPARENT — required by the current render order
        // (`WorkbenchEguiHost` Camera2d order=1, Camera3d order=0).
        // Camera3d paints 3D first; the host then alpha-composites egui chrome.
        // If this panel painted an opaque backdrop, egui would overpaint the
        // 3D pixels in the centre. Bleed safety isn't lost — the camera
        // reconciler makes the scene camera inactive when ViewportPanel isn't
        // in the active layout, so no 3D reaches the presentation target then.
        true
    }

    fn scroll_policy(&self) -> PanelScrollPolicy {
        PanelScrollPolicy::SelfManaged
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        // Record the live viewport rect for the occlusion/picking gate. The
        // scene camera remains full-window so transparent dock content can
        // reveal it; scene-vs-chrome picking is handled by bevy_picking (egui
        // occlusion via bevy_egui's picking backend), so there's no pointer
        // gate to compute here anymore.
        //
        // Measure the rect now (needs `ui`), then emit a typed measurement
        // intent — render has no mutable world access.
        let rect = PanelRects::panel_rect_from_ui(ui);
        // Authoritative scene-vs-chrome signal — egui's own occlusion-aware hit
        // test, measured now (needs `ui`), folded after the paint.
        let over_scene = ScenePickGate::scene_pointer_from_ui(ui);
        ctx.trigger(ViewportPanelMeasured { rect, over_scene });
        // Reserve the panel's space so egui_dock's layout accounts for
        // it; no widgets are drawn — the 3D camera paints here.
        ui.allocate_space(ui.available_size());
    }
}

/// Keep the egui host's main-texture key equal to the active scene camera's.
///
/// Bevy keys a render target's intermediate textures by `(target, usages,
/// format, msaa)`. The host deliberately shares the scene camera's main
/// texture so egui paints directly over the scene pixels that were cleared in
/// the lower-order camera. MSAA and HDR are part of that key; both must follow
/// the active scene camera rather than being presentation constants.
///
/// This is change-driven and covers every owner transition: camera quality
/// changes, camera replacement/activation, a newly bound scene camera, and
/// scene-camera removal. If no active window scene camera exists, the layout
/// paints its own opaque backdrop and there is no valid scene texture to share.
pub(crate) fn sync_egui_host_msaa(
    dirty: Query<
        (),
        (
            With<SceneCamera>,
            Without<WorkbenchEguiHost>,
            Or<(
                Changed<Msaa>,
                Changed<Camera>,
                Changed<Hdr>,
                Added<SceneCamera>,
            )>,
        ),
    >,
    mut removed: RemovedComponents<SceneCamera>,
    host_added: Query<(), Added<WorkbenchEguiHost>>,
    scene_cams: Query<
        (Entity, &Camera, &Msaa, &RenderTarget, Has<Hdr>),
        (With<SceneCamera>, Without<WorkbenchEguiHost>),
    >,
    viewport: Option<Res<SceneViewport>>,
    mut host: Query<(Entity, &mut Msaa, Has<Hdr>), (With<WorkbenchEguiHost>, Without<SceneCamera>)>,
    mut commands: Commands,
) {
    let had_removal = removed.read().count() > 0;
    if dirty.is_empty() && host_added.is_empty() && !had_removal {
        return;
    }

    let is_window = |target: &RenderTarget| matches!(target, RenderTarget::Window(_));
    let Some(active_camera) = viewport.as_deref().and_then(|vp| vp.active_camera) else {
        return;
    };
    let want = scene_cams
        .get(active_camera)
        .ok()
        .filter(|(_, camera, _, target, _)| camera.is_active && is_window(target))
        .map(|(_, _, msaa, _, hdr)| (*msaa, hdr));
    let Some((want_msaa, want_hdr)) = want else {
        return;
    };

    for (entity, mut msaa, host_hdr) in &mut host {
        if *msaa != want_msaa {
            *msaa = want_msaa;
        }
        if want_hdr != host_hdr {
            if want_hdr {
                commands.entity(entity).try_insert(Hdr);
            } else {
                commands.entity(entity).remove::<Hdr>();
            }
        }
    }
}

/// Construct the Bevy camera used by the persistent egui host.
fn egui_host_camera() -> Camera {
    Camera {
        // The scene camera is order 0; egui paints into the shared main texture
        // after it. Its clear operation must remain disabled so it cannot wipe
        // the scene before the UI pass.
        order: 1,
        clear_color: ClearColorConfig::None,
        ..default()
    }
}

/// Startup system — auto-spawn one [`WorkbenchEguiHost`] if none exists.
///
/// Always disables `EguiGlobalSettings::auto_create_primary_context` so
/// bevy_egui cannot choose a different camera as primary. Idempotent: re-running
/// will not spawn duplicates.
pub(crate) fn ensure_egui_host(
    mut commands: Commands,
    mut egui_global: ResMut<EguiGlobalSettings>,
    existing: Query<(), With<PrimaryEguiContext>>,
    bindings: Res<InputBindingsSettings>,
) {
    egui_global.auto_create_primary_context = false;
    if existing.iter().next().is_none() {
        let input_map = bindings
            .input_map()
            .expect("registered input bindings must satisfy their settings contract");
        commands.spawn((
            Camera2d,
            // `order = 1` places egui strictly after the scene Camera3d.
            // `egui_host_camera` keeps the UI pass on the scene camera's shared
            // main texture; `sync_egui_host_msaa` maintains the texture key.
            egui_host_camera(),
            //
            // Render NO world layers. This camera exists ONLY to paint egui chrome
            // (a render pass that ignores `RenderLayers`); it must not also run the
            // gizmo pass. A default (layer-0) Camera2d would re-project every 3D
            // world gizmo through its 2D orthographic view onto the UI target.
            // Excluding the world layers keeps gizmos on the scene `Camera3d` only.
            RenderLayers::none(),
            PrimaryEguiContext,
            WorkbenchEguiHost,
            LocalIntentSurface,
            IntentState::default(),
            input_map,
            Name::new("WorkbenchEguiHost"),
        ));
    }
}

/// Push the workbench's scene visibility into [`SceneViewport`](lunco_core::SceneViewport).
/// The scene camera remains full-window; the measured dock leaf is independent
/// geometry consumed by the occlusion/picking gate. This system deliberately
/// does NOT touch `Camera::is_active` or `Camera::viewport` so the workbench and
/// the camera switch stop fighting over them.
pub(crate) fn apply_workbench_viewport(
    layout: Option<Res<crate::WorkbenchLayout>>,
    vp: Option<ResMut<lunco_core::SceneViewport>>,
) {
    // The workbench contributes data only; `lunco-usd-bevy` remains the single
    // authority that actuates `Camera::is_active` and `Camera::viewport`.
    let (visible, rect) = resolve_scene_viewport_layout(layout.as_deref());
    let Some(mut vp) = vp else { return };
    if vp.visible != visible {
        vp.visible = visible;
    }
    if vp.rect != rect {
        vp.rect = rect;
    }
}

/// Resolve the workbench's contribution to the scene viewport without touching
/// ECS state. A scene-hosting layout keeps the scene camera full-window so
/// transparent dock content can reveal it. The viewport leaf remains a UI
/// measurement for `PanelRects` and `ScenePickGate`, not a render crop.
fn resolve_scene_viewport_layout(
    layout: Option<&crate::WorkbenchLayout>,
) -> (bool, Option<(UVec2, UVec2)>) {
    let (layout_empty, layout_has_viewport, scene_visible_when_docked) = match layout {
        None => (true, false, false),
        Some(l) => (
            layout_is_empty(l),
            layout_contains_panel(l, VIEWPORT_PANEL_ID),
            l.active_perspective_scene_visible_when_docked(),
        ),
    };
    if layout_empty || scene_visible_when_docked {
        return (true, None);
    }
    if !layout_has_viewport {
        return (false, None);
    }
    (true, None)
}

/// True iff `panel` appears in the active layout — either as a tab in
/// the dock or in one of the four slot Vecs the perspectives populate.
///
/// `dock.iter_all_tabs()` alone isn't enough: a perspective plan writes
/// to the slot Vecs first; the dock is *rebuilt from those slots* by
/// `rebuild_dock`. In steady state both contain the same panels, but
/// pinning the camera-active decision to layout membership (rather than
/// `PanelRects` which keeps stale rects on purpose) is what makes the
/// "is the viewport even part of this perspective?" question
/// authoritative.
/// True iff every slot Vec is empty AND the dock has no *singleton panel*
/// tabs — a View-style perspective that wants the entire window for the
/// 3D scene with no chrome painted on top.
///
/// Parked *instance* tabs (open documents/models) are deliberately
/// ignored: a hybrid app (the rover luncosim embeds the Modelica
/// workbench) can have documents open while a viewport-only perspective
/// is active. `rebuild_dock` parks those instance tabs in the dock so
/// they survive and re-attach on switch, and `render_layout` keeps the
/// workbench in 3D mode (it gates on the centre intent, not the dock).
/// They never paint chrome here, so the camera must stay full-window —
/// counting them as "non-empty" would wrongly flip the camera inactive
/// (the Design-style "panels but no viewport" branch) and blank the 3D.
pub(crate) fn layout_is_empty(layout: &crate::WorkbenchLayout) -> bool {
    layout.side_browser.is_empty()
        && layout.side_browser_bottom.is_empty()
        && layout.center.is_empty()
        && layout.right_inspector.is_empty()
        && layout.right_inspector_bottom.is_empty()
        && layout.bottom.is_empty()
        && !layout
            .dock
            .iter_all_tabs()
            .any(|(_, t)| matches!(t, lunco_workbench_core::TabId::Singleton(_)))
}

pub(crate) fn layout_contains_panel(layout: &crate::WorkbenchLayout, panel: PanelId) -> bool {
    if layout.side_browser.contains(&panel)
        || layout.side_browser_bottom.contains(&panel)
        || layout.center.contains(&panel)
        || layout.right_inspector.contains(&panel)
        || layout.right_inspector_bottom.contains(&panel)
        || layout.bottom.contains(&panel)
    {
        return true;
    }
    layout
        .dock
        .iter_all_tabs()
        .any(|(_, t)| matches!(t, lunco_workbench_core::TabId::Singleton(id) if *id == panel))
}

/// Sentinel — runs each frame on newly-added Camera3d entities and
/// warns when one targets the window without the canonical [`SceneCamera`]
/// intent.
///
/// Catches the entire regression class at the ownership boundary: any future
/// code path that constructs a concrete window `Camera3d` without first
/// declaring a render-free `SceneCamera` is incomplete. The Workbench does not
/// infer or adopt that camera, because doing so would hide the missing owner
/// from every other camera consumer.
///
/// The check is per-`Added<Camera3d>` rather than a periodic sweep so
/// USD/avatar-spawned cameras (which can land many frames after
/// startup) are still validated, and so deleting + respawning the host
/// during teardown doesn't yield false negatives.
pub(crate) fn check_camera_invariants(
    new_cams: Query<(Entity, Option<&RenderTarget>), (Added<Camera3d>, Without<SceneCamera>)>,
) {
    for (entity, target) in &new_cams {
        let targets_window = matches!(target, None | Some(RenderTarget::Window(_)));
        if targets_window {
            // Warn loudly but don't panic: a concrete window camera without
            // the intent has no declared owner. If a tooling binary really
            // wants a full-window camera, it should still declare SceneCamera;
            // that is the shared contract for render, switching and viewport
            // ownership.
            warn!(
                "WorkbenchPlugin: Camera3d {entity:?} targets the window without \
                 `lunco_render::SceneCamera`; declare the camera intent at its \
                 spawn/translation site instead of relying on Workbench inference."
            );
        }
    }
}

/// Sentinel — runs once a couple of seconds after startup and verifies
/// there's exactly one `PrimaryEguiContext` in the world.
///
/// The grace period covers binaries that spawn the host asynchronously —
/// they'll have done so by the time this
/// fires. After that, anything other than 1 is a bug worth panicking
/// over in debug builds.
pub(crate) fn check_host_invariant_once(
    hosts: Query<(), With<PrimaryEguiContext>>,
    time: Res<Time>,
    mut done: Local<bool>,
) {
    if *done || time.elapsed_secs() < 1.0 {
        return;
    }
    *done = true;
    let n = hosts.iter().count();
    if n != 1 {
        warn!(
            "WorkbenchPlugin: expected exactly 1 `PrimaryEguiContext`, found {n}. \
             Was a stray (Camera2d, PrimaryEguiContext) spawned outside \
             `ensure_egui_host`? See `lunco-workbench/src/viewport.rs`."
        );
    }
}

/// Turn OFF bevy_egui's built-in pointer-capture backend on the primary egui
/// context. Its `capture_pointer_input_system` emits a top-priority
/// `bevy_picking` hit over the WHOLE egui context viewport whenever egui
/// `wants_pointer_input()` — and in egui-dock "Build" mode the central
/// `ViewportPanel` leaf makes egui want the pointer over the 3D region, so that
/// blanket capture suppressed every scene pick (clicks never reached the 3D).
/// We replace it with [`egui_viewport_aware_picking`], which captures only over
/// real chrome and never over the live viewport rect. Idempotent; the change
/// guard keeps it from dirtying the component every frame.
pub(crate) fn disable_egui_pointer_capture(
    mut q: Query<&mut bevy_egui::EguiContextSettings, With<PrimaryEguiContext>>,
) {
    for mut s in q.iter_mut() {
        if s.capture_pointer_input {
            s.capture_pointer_input = false;
        }
    }
}

/// Clear the pick gate's per-frame inputs. Runs in `First`, **unconditionally**,
/// so the reset happens on every frame — including frames where the egui pass is
/// skipped (window occluded / minimized, or a host that doesn't call
/// `render_workbench`). That is what makes the gate's input lifetime honest:
/// `rendered == false` then tells [`resolve_scene_pointer`] to hold its previous
/// answer rather than resolve against empty inputs.
pub(crate) fn reset_scene_pick_gate(mut gate: ResMut<ScenePickGate>) {
    gate.begin_frame();
}

/// Collapse egui's own geometry + this frame's dock/panel rects into the single
/// resolved [`SceneTarget`] that [`track_egui_focus`] (→ `EguiFocus.wants_pointer`)
/// and [`egui_viewport_aware_picking`] read. Runs in PostUpdate after the egui pass,
/// ordered BEFORE both consumers.
///
/// The decision and press-latch live in the pure
/// `lunco_workbench_core::scene_pick` contract. This system's only job is to
/// read egui's `Context` and hand that contract the inputs.
pub(crate) fn resolve_scene_pointer(
    mut gate: ResMut<ScenePickGate>,
    mut q: Query<&mut bevy_egui::EguiContext, With<PrimaryEguiContext>>,
) {
    let mut state = EguiPointerState::default();
    for mut ctx in q.iter_mut() {
        let c = ctx.get_mut();
        state.over_egui |= c.is_pointer_over_egui();
        state.using_pointer |= c.egui_is_using_pointer();
        // `pointer_hover_pos()` — NOT `pointer_interact_pos()`, which egui keeps
        // alive after `PointerGone` and would have the gate resolving against a
        // cursor that has left the window entirely.
        state.hover_pos = state.hover_pos.or_else(|| c.pointer_hover_pos());
        state.any_down |= c.input(|i| i.pointer.any_down());
    }
    let scene_press = gate.resolve(state);
    if scene_press {
        // A scene click is the explicit handoff from an egui editor/control to
        // the interactive simulation. egui retains the last focused TextEdit
        // even after its panel is hidden by a perspective switch; surrendering
        // that focus at the scene press keeps the shared keyboard map live for
        // possession and vessel control without weakening text-field capture.
        for mut ctx in q.iter_mut() {
            ctx.get_mut().memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
        }
    }
}

/// Scene-vs-chrome-aware egui picking backend — the replacement for bevy_egui's
/// blanket capture (disabled by [`disable_egui_pointer_capture`]).
///
/// Emits a high-order `bevy_picking` hit for the egui context entity ONLY when the
/// pointer is over chrome, i.e. NOT over the main 3D scene. It reads the single
/// resolved [`ScenePickGate::over_main_scene`] computed by [`resolve_scene_pointer`]
/// (which runs just before this), so:
///   • over chrome → egui hit wins → the 3D pick is suppressed;
///   • over an offscreen preview panel (USD) → still a capture hit: that panel owns
///     its own input and mesh hits must not fire *behind* its image;
///   • over the main scene (viewport leaf, full-window View centre) → no egui hit →
///     bevy_picking's mesh hit reaches the scene observers.
/// Mirrors bevy_egui's own `capture_pointer_input_system` (PostUpdate, hit with no
/// world position so consumers can tell chrome from a real mesh pick). Unlike the
/// stock capture it does NOT gate on `egui_wants_pointer_input()` (button-masked —
/// see [`resolve_scene_pointer`]); the resolved signal is unconditional.
pub(crate) fn egui_viewport_aware_picking(
    pointers: Query<(
        &bevy::picking::pointer::PointerId,
        &bevy::picking::pointer::PointerLocation,
    )>,
    egui_q: Query<(Entity, &Camera), With<PrimaryEguiContext>>,
    gate: Res<ScenePickGate>,
    picking_order: Option<Res<bevy_egui::EguiPickingOrder>>,
    mut out: MessageWriter<bevy::picking::backend::PointerHits>,
) {
    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::backend::{HitData, PointerHits};

    let extra = picking_order.map(|o| o.0).unwrap_or(0.6);
    for (id, loc) in pointers
        .iter()
        .filter_map(|(i, p)| p.location.as_ref().map(|l| (i, l)))
    {
        let NormalizedRenderTarget::Window(_) = loc.target else {
            continue;
        };
        for (entity, camera) in egui_q.iter() {
            // Pointer must be inside the egui camera's viewport (full window).
            let Some(vp) = camera.physical_viewport_rect() else {
                continue;
            };
            if !vp.as_rect().contains(loc.position) {
                continue;
            }
            // `resolve_scene_pointer` (runs just before this) has already collapsed
            // scene-vs-chrome into one `SceneTarget`. Over the main scene → let the
            // mesh pick win (no capture). Otherwise → emit a high-order capture hit
            // UNCONDITIONALLY. We must NOT gate on `egui_wants_pointer_input()`
            // here: its `&& !any_down()` clause returns false at the instant of a
            // press over a panel background/label, which let the gizmo/hover pick
            // leak through chrome on click.
            if gate.over_main_scene() {
                continue;
            }
            out.write(PointerHits::new(
                *id,
                vec![(entity, HitData::new(entity, 0.0, None, None))],
                camera.order as f32 + extra,
            ));
        }
    }
}

/// Relay egui's input-capture flags into the ECS as [`lunco_control_core::EguiFocus`].
///
/// egui reads its own copy of the winit events and never removes anything from
/// Bevy's `ButtonInput`, so raw scene-input systems (keyboard driving, camera
/// orbit, scroll-zoom) would otherwise fire even while an egui text field is
/// focused or the pointer is over a panel. This publishes `wants_keyboard` (from
/// egui's `wants_keyboard_input()`) and `wants_pointer` (the negation of the
/// resolved [`ScenePickGate::over_main_scene`] — see [`resolve_scene_pointer`],
/// NOT the button-masked `egui_wants_pointer_input()`) so those systems can gate
/// on `EguiFocus` without depending on `bevy_egui`.
///
/// Runs in `PostUpdate` after `EguiPostUpdateSet::ProcessOutput` (same slot as
/// the picking backend) so the flags are this-frame-fresh; consumers in `Update`
/// read them one frame later, which is imperceptible for held input.
pub(crate) fn track_egui_focus(
    mut focus: ResMut<lunco_control_core::EguiFocus>,
    mut q: Query<&mut bevy_egui::EguiContext, With<PrimaryEguiContext>>,
    gate: Res<ScenePickGate>,
) {
    // Pointer gate: egui owns the pointer whenever it is NOT over the main 3D
    // scene. The gate has already resolved that (this-frame, mode-aware,
    // occlusion-aware) AND latched ownership on press, so a drag that started on an
    // egui widget keeps the pointer for its whole duration even as the cursor
    // travels over the viewport — and vice versa. We do NOT use
    // `egui_wants_pointer_input()`: its `&& !any_down()` clause drops to false at
    // the instant of a press over a panel background/label, which leaked scene picks
    // / camera-orbit through chrome on click.
    let ptr = !gate.over_main_scene();

    // Keyboard still comes straight from egui focus (a focused text field).
    let mut kb = false;
    for mut ctx in q.iter_mut() {
        kb |= ctx.get_mut().egui_wants_keyboard_input();
    }

    // Change-guarded so the resource isn't dirtied every frame.
    if focus.wants_keyboard != kb || focus.wants_pointer != ptr {
        focus.wants_keyboard = kb;
        focus.wants_pointer = ptr;
    }
}

/// Sub-plugin auto-added by `WorkbenchPlugin`. Wires the egui host,
/// the panel-rect tracking, the `Camera::viewport` sync, and the
/// invariant sentinels.
pub struct WorkbenchViewportPlugin;

fn apply_viewport_panel_measurement(
    trigger: On<ViewportPanelMeasured>,
    mut rects: ResMut<PanelRects>,
    mut gate: ResMut<ScenePickGate>,
) {
    rects.record(VIEWPORT_PANEL_ID, trigger.rect);
    gate.record_scene_leaf(SceneTarget::MainViewport, trigger.over_scene);
}

impl Plugin for WorkbenchViewportPlugin {
    fn build(&self, app: &mut App) {
        lunco_control_core::ensure_control_plugin(app);
        if !app.is_plugin_added::<lunco_input_core::InputBindingsPlugin>() {
            app.add_plugins(lunco_input_core::InputBindingsPlugin);
        }
        app.init_resource::<PanelRects>()
            .init_resource::<ScenePickGate>()
            .init_resource::<ViewportPlaceholder>()
            .add_observer(apply_viewport_panel_measurement)
            .add_systems(Startup, ensure_egui_host)
            // Clear the pick gate's per-frame inputs. `First` — NOT the egui pass —
            // because this must happen even on frames where the egui pass is
            // skipped; that's what lets `resolve_scene_pointer` tell "no inputs"
            // from "stale inputs" and hold instead of resolving against garbage.
            // (`PanelRects` is deliberately NOT cleared here: its consumers run in
            // `Update`, before the egui pass writes it. It is cleared at the top of
            // `render_workbench` instead.)
            .add_systems(First, reset_scene_pick_gate)
            .add_systems(
                Update,
                (
                    check_camera_invariants,
                    check_host_invariant_once,
                    sync_egui_host_msaa,
                ),
            )
            // Keep bevy_egui's blanket pointer-capture OFF; we provide our own
            // viewport-aware backend instead (so the egui-dock ViewportPanel leaf
            // doesn't suppress 3D picking over the scene).
            .add_systems(Update, disable_egui_pointer_capture)
            // Resolve the per-frame scene-vs-chrome signal from the render fold +
            // egui geometry. PostUpdate after the egui pass, BEFORE the two
            // consumers below (which read the resolved `pointer_over_scene`).
            .add_systems(
                PostUpdate,
                resolve_scene_pointer
                    .after(bevy_egui::EguiPostUpdateSet::ProcessOutput)
                    .before(egui_viewport_aware_picking)
                    .before(track_egui_focus),
            )
            // Custom egui picking backend. PostUpdate after the egui pass so the
            // resolved scene-vs-chrome signal is this-frame fresh (mirrors
            // bevy_egui's own capture system's schedule).
            .add_systems(
                PostUpdate,
                egui_viewport_aware_picking.after(bevy_egui::EguiPostUpdateSet::ProcessOutput),
            )
            // Publish the pointer/keyboard gate into `EguiFocus` (same post-egui
            // slot) so raw scene-input systems can gate on it. See `track_egui_focus`.
            .add_systems(
                PostUpdate,
                track_egui_focus.after(bevy_egui::EguiPostUpdateSet::ProcessOutput),
            )
            .add_systems(
                PostUpdate,
                apply_workbench_viewport.in_set(lunco_core::SceneViewportSet::Publish),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::CameraOutputMode;

    #[test]
    fn egui_host_camera_uses_the_shared_scene_target_contract() {
        let camera = egui_host_camera();

        assert_eq!(camera.order, 1);
        assert!(matches!(camera.clear_color, ClearColorConfig::None));
        assert!(matches!(
            camera.output_mode,
            CameraOutputMode::Write {
                blend_state: None,
                clear_color: ClearColorConfig::Default,
            }
        ));
    }

    // ── SceneViewport layout contribution ──────────────────────────────────

    #[test]
    fn docked_viewport_keeps_scene_full_window() {
        let mut layout = crate::WorkbenchLayout::default();
        layout.center.push(VIEWPORT_PANEL_ID);

        let (visible, rect) = resolve_scene_viewport_layout(Some(&layout));

        assert!(visible);
        assert_eq!(rect, None);
    }

    #[test]
    fn docked_viewport_does_not_wait_for_leaf_measurement() {
        let mut layout = crate::WorkbenchLayout::default();
        layout.center.push(VIEWPORT_PANEL_ID);

        let (visible, rect) = resolve_scene_viewport_layout(Some(&layout));

        assert!(visible);
        assert_eq!(rect, None);
    }

    #[test]
    fn design_layout_never_paints_scene() {
        let mut layout = crate::WorkbenchLayout::default();
        layout.center.push(PanelId("lunica::diagram"));

        let (visible, rect) = resolve_scene_viewport_layout(Some(&layout));

        assert!(!visible);
        assert_eq!(rect, None);
    }

    struct SceneBackedPerspective;

    impl lunco_workbench_core::Perspective for SceneBackedPerspective {
        fn id(&self) -> lunco_workbench_core::PerspectiveId {
            lunco_workbench_core::PerspectiveId("scene_backed")
        }

        fn title(&self) -> String {
            "Scene backed".into()
        }

        fn scene_visible_when_docked(&self) -> bool {
            true
        }

        fn layout(&self) -> lunco_workbench_core::PerspectiveLayoutPlan {
            lunco_workbench_core::PerspectiveLayoutPlan::new()
        }
    }

    #[test]
    fn scene_backed_perspective_keeps_scene_with_transient_panel() {
        let mut layout = crate::WorkbenchLayout::default();
        layout.register_perspective(SceneBackedPerspective);
        layout.right_inspector.push(PanelId("command_deck"));

        let (visible, rect) = resolve_scene_viewport_layout(Some(&layout));

        assert!(visible);
        assert_eq!(rect, None);
    }
}
