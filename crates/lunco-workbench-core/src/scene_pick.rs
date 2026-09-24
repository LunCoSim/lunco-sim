//! Generic scene-vs-chrome pointer ownership contracts.
//!
//! The concrete workbench records dock and egui geometry, while scene-aware
//! consumers read the resolved owner. Keeping the state machine here lets
//! offscreen scene panels depend on the contract without depending on the
//! concrete dock shell.

use bevy::prelude::Resource;

use crate::PanelId;

/// Which live 3D scene a pointer position belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneTarget {
    /// The full-window 3D scene hosted by the application's viewport panel.
    MainViewport,
    /// A panel-owned scene rendered to an offscreen image.
    Offscreen(PanelId),
}

/// Per-frame inputs and resolved output of the scene-vs-chrome pick gate.
///
/// The concrete host resets the gate before its egui pass, records scene and
/// chrome geometry while rendering, then calls [`ScenePickGate::resolve`] after
/// egui has produced its pointer state. If the egui pass is skipped, the gate
/// holds its previous answer instead of resolving against empty inputs.
///
/// All rects here are egui points (not physical pixels; those are held by
/// [`crate::viewport::PanelRects`]).
#[derive(Resource, Default, Debug)]
pub struct ScenePickGate {
    rendered: bool,
    scene_leaf: Option<SceneTarget>,
    chrome_cards: Vec<(egui::Rect, egui::Rect)>,
    dock_rect: Option<egui::Rect>,
    scene_viewport_rect: Option<egui::Rect>,
    resolved: Option<SceneTarget>,
    tool_pointer_capture: bool,
    latched: bool,
}

/// The egui-geometry half of the gate's per-frame inputs.
#[derive(Debug, Clone, Copy, Default)]
pub struct EguiPointerState {
    /// Whether egui's occlusion-aware pointer test finds a reserved surface.
    pub over_egui: bool,
    /// Whether egui is actively dragging one of its own widgets.
    pub using_pointer: bool,
    /// The pointer position, or `None` after the pointer leaves the window.
    pub hover_pos: Option<egui::Pos2>,
    /// Whether any pointer button is currently held.
    pub any_down: bool,
}

impl ScenePickGate {
    /// Test whether the pointer is inside a scene panel's occlusion-aware egui
    /// content area.
    pub fn scene_pointer_from_ui(ui: &egui::Ui) -> bool {
        ui.rect_contains_pointer(ui.available_rect_before_wrap())
    }

    /// Record that the pointer is inside `target`'s scene leaf this frame.
    pub fn record_scene_leaf(&mut self, target: SceneTarget, over: bool) {
        if over {
            self.scene_leaf = Some(target);
        }
    }

    /// Record a docked chrome panel's blocked region in egui points.
    pub fn record_chrome_panel(&mut self, body: egui::Rect, card: egui::Rect) {
        self.chrome_cards.push((body, card));
    }

    /// Record the dock's extent in egui points.
    pub fn set_dock_rect(&mut self, rect: egui::Rect) {
        self.dock_rect = Some(rect);
    }

    /// Record the viewport leaf's layout rect in egui points.
    pub fn set_scene_viewport_rect(&mut self, rect: Option<egui::Rect>) {
        self.scene_viewport_rect = rect;
    }

    /// Record that an interactive tool owns the current primary drag.
    pub fn set_tool_pointer_capture(&mut self, captured: bool) {
        self.tool_pointer_capture = captured;
    }

    /// Mark that the host's egui pass ran this frame.
    pub fn mark_rendered(&mut self) {
        self.rendered = true;
    }

    /// Clear the host-provided per-frame inputs.
    pub fn begin_frame(&mut self) {
        self.rendered = false;
        self.scene_leaf = None;
        self.chrome_cards.clear();
        self.dock_rect = None;
        self.scene_viewport_rect = None;
        self.tool_pointer_capture = false;
    }

    /// Return the resolved scene under the pointer, or `None` for chrome.
    pub fn resolved(&self) -> Option<SceneTarget> {
        self.resolved
    }

    /// Return whether the pointer owns the main full-window 3D scene.
    pub fn over_main_scene(&self) -> bool {
        self.resolved == Some(SceneTarget::MainViewport)
    }

    /// Return whether an interactive tool owns the primary drag in an offscreen
    /// preview.
    pub fn tool_pointer_capture(&self) -> bool {
        self.tool_pointer_capture
    }

    /// Fold the host-provided geometry and egui pointer state into the resolved
    /// target, applying the press latch. Returns `true` for a newly recognized
    /// main-scene press.
    pub fn resolve(&mut self, egui_state: EguiPointerState) -> bool {
        if !self.rendered {
            return false;
        }
        let candidate = resolve_scene_target(
            egui_state,
            self.scene_leaf,
            &self.chrome_cards,
            self.dock_rect,
            self.scene_viewport_rect,
        );
        let latch = PressLatch {
            held: self.latched,
            owner: self.resolved,
        };
        let scene_press =
            !self.latched && egui_state.any_down && candidate == Some(SceneTarget::MainViewport);
        let next = latch.update(egui_state.any_down, candidate);
        self.resolved = next.owner;
        self.latched = next.held;
        scene_press
    }
}

fn resolve_scene_target(
    egui_state: EguiPointerState,
    scene_leaf: Option<SceneTarget>,
    chrome_cards: &[(egui::Rect, egui::Rect)],
    dock_rect: Option<egui::Rect>,
    scene_viewport_rect: Option<egui::Rect>,
) -> Option<SceneTarget> {
    let pos = egui_state.hover_pos?;
    // Runtime HUI controls are registered as chrome cards. They must own the
    // pointer before the full-window scene leaf is considered; the leaf is a
    // geometry hit target, not an input capture layer.
    if chrome_cards.iter().any(|(_, card)| card.contains(pos)) {
        return None;
    }
    if let Some(target) = scene_leaf {
        return Some(target);
    }
    if egui_state.using_pointer || egui_state.over_egui {
        return None;
    }
    if scene_viewport_rect.is_some_and(|rect| rect.contains(pos)) {
        return Some(SceneTarget::MainViewport);
    }
    if chrome_cards
        .iter()
        .any(|(body, card)| body.contains(pos) && !card.contains(pos))
    {
        return Some(SceneTarget::MainViewport);
    }
    if dock_rect.is_some_and(|rect| rect.contains(pos)) {
        return None;
    }
    Some(SceneTarget::MainViewport)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PressLatch {
    held: bool,
    owner: Option<SceneTarget>,
}

impl PressLatch {
    fn update(self, any_down: bool, candidate: Option<SceneTarget>) -> Self {
        match (any_down, self.held) {
            (false, _) => Self {
                held: false,
                owner: candidate,
            },
            (true, false) => Self {
                held: true,
                owner: candidate,
            },
            (true, true) => Self {
                held: true,
                owner: self.owner,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Rect, pos2};

    const USD_PREVIEW: PanelId = PanelId("usd::viewport");

    fn rect(min: (f32, f32), max: (f32, f32)) -> Rect {
        Rect::from_min_max(pos2(min.0, min.1), pos2(max.0, max.1))
    }

    fn hovering(p: (f32, f32)) -> EguiPointerState {
        EguiPointerState {
            hover_pos: Some(pos2(p.0, p.1)),
            ..Default::default()
        }
    }

    #[test]
    fn below_the_dock_is_scene() {
        let dock = rect((0.0, 30.0), (800.0, 400.0));
        let cards = [(
            rect((0.0, 30.0), (200.0, 400.0)),
            rect((0.0, 30.0), (200.0, 400.0)),
        )];
        assert_eq!(
            resolve_scene_target(hovering((400.0, 500.0)), None, &cards, Some(dock), None),
            Some(SceneTarget::MainViewport)
        );
    }

    #[test]
    fn dock_furniture_is_chrome() {
        let dock = rect((0.0, 30.0), (800.0, 400.0));
        let cards = [(
            rect((0.0, 60.0), (200.0, 400.0)),
            rect((0.0, 60.0), (200.0, 400.0)),
        )];
        assert_eq!(
            resolve_scene_target(hovering((100.0, 40.0)), None, &cards, Some(dock), None),
            None
        );
    }

    #[test]
    fn full_window_mode_is_all_scene() {
        assert_eq!(
            resolve_scene_target(hovering((400.0, 300.0)), None, &[], None, None),
            Some(SceneTarget::MainViewport)
        );
    }

    #[test]
    fn main_scene_press_is_reported_once() {
        let mut gate = ScenePickGate::default();
        gate.begin_frame();
        gate.mark_rendered();
        gate.record_scene_leaf(SceneTarget::MainViewport, true);
        let mut press = hovering((400.0, 300.0));
        press.any_down = true;
        assert!(gate.resolve(press));
        assert!(gate.over_main_scene());

        gate.begin_frame();
        gate.mark_rendered();
        gate.record_scene_leaf(SceneTarget::MainViewport, true);
        assert!(!gate.resolve(press));
    }

    #[test]
    fn opaque_panel_has_no_transparent_gap() {
        let body = rect((0.0, 30.0), (200.0, 400.0));
        assert_eq!(
            resolve_scene_target(
                hovering((100.0, 380.0)),
                None,
                &[(body, body)],
                Some(body),
                None
            ),
            None
        );
    }

    #[test]
    fn transparent_panel_gap_is_scene() {
        let body = rect((0.0, 30.0), (200.0, 400.0));
        let card = rect((0.0, 30.0), (200.0, 120.0));
        assert_eq!(
            resolve_scene_target(
                hovering((100.0, 60.0)),
                None,
                &[(body, card)],
                Some(body),
                None
            ),
            None
        );
        assert_eq!(
            resolve_scene_target(
                hovering((100.0, 300.0)),
                None,
                &[(body, card)],
                Some(body),
                None
            ),
            Some(SceneTarget::MainViewport)
        );
    }

    #[test]
    fn scene_leaf_wins_over_egui_and_widget_drag() {
        let mut state = hovering((400.0, 200.0));
        state.over_egui = true;
        state.using_pointer = true;
        assert_eq!(
            resolve_scene_target(
                state,
                Some(SceneTarget::MainViewport),
                &[],
                Some(rect((0.0, 30.0), (800.0, 400.0))),
                None,
            ),
            Some(SceneTarget::MainViewport)
        );
    }

    #[test]
    fn runtime_ui_control_owns_pointer_over_full_window_scene_leaf() {
        let control = rect((720.0, 40.0), (860.0, 80.0));
        assert_eq!(
            resolve_scene_target(
                hovering((760.0, 60.0)),
                Some(SceneTarget::MainViewport),
                &[(control, control)],
                None,
                None,
            ),
            None
        );
        assert_eq!(
            resolve_scene_target(
                hovering((900.0, 60.0)),
                Some(SceneTarget::MainViewport),
                &[(control, control)],
                None,
                None,
            ),
            Some(SceneTarget::MainViewport)
        );
    }

    #[test]
    fn offscreen_preview_is_not_the_main_scene() {
        let body = rect((200.0, 60.0), (800.0, 400.0));
        let target = Some(SceneTarget::Offscreen(USD_PREVIEW));
        let out = resolve_scene_target(
            hovering((400.0, 200.0)),
            target,
            &[(body, body)],
            Some(rect((0.0, 30.0), (800.0, 400.0))),
            None,
        );
        assert_eq!(out, target);
        assert_ne!(out, Some(SceneTarget::MainViewport));
    }

    #[test]
    fn collapsed_viewport_leaf_rect_is_scene() {
        let dock = rect((0.0, 30.0), (800.0, 400.0));
        let viewport = rect((200.0, 30.0), (800.0, 400.0));
        assert_eq!(
            resolve_scene_target(
                hovering((400.0, 200.0)),
                None,
                &[],
                Some(dock),
                Some(viewport)
            ),
            Some(SceneTarget::MainViewport)
        );
    }

    #[test]
    fn opaque_panel_over_collapsed_viewport_is_chrome() {
        let dock = rect((0.0, 30.0), (800.0, 400.0));
        let viewport = rect((0.0, 30.0), (800.0, 400.0));
        let body = rect((0.0, 30.0), (200.0, 400.0));
        assert_eq!(
            resolve_scene_target(
                hovering((100.0, 200.0)),
                None,
                &[(body, body)],
                Some(dock),
                Some(viewport),
            ),
            None
        );
    }

    #[test]
    fn latch_keeps_widget_drag_in_chrome() {
        let mut latch = PressLatch::default();
        latch = latch.update(false, None);
        latch = latch.update(true, None);
        latch = latch.update(true, Some(SceneTarget::MainViewport));
        assert_eq!(latch.owner, None);
        assert!(latch.held);
        assert_eq!(
            latch.update(false, Some(SceneTarget::MainViewport)),
            PressLatch {
                held: false,
                owner: Some(SceneTarget::MainViewport),
            }
        );
    }

    #[test]
    fn latch_keeps_scene_drag_in_scene() {
        let mut latch = PressLatch::default();
        latch = latch.update(false, Some(SceneTarget::MainViewport));
        latch = latch.update(true, Some(SceneTarget::MainViewport));
        latch = latch.update(true, None);
        assert_eq!(latch.owner, Some(SceneTarget::MainViewport));
        assert_eq!(
            latch.update(false, None),
            PressLatch {
                held: false,
                owner: None,
            }
        );
    }

    #[test]
    fn skipped_egui_frame_holds_last_answer() {
        let mut gate = ScenePickGate::default();
        gate.begin_frame();
        gate.mark_rendered();
        gate.record_scene_leaf(SceneTarget::MainViewport, true);
        gate.resolve(hovering((400.0, 300.0)));
        assert!(gate.over_main_scene());

        gate.begin_frame();
        gate.resolve(EguiPointerState::default());
        assert!(gate.over_main_scene());
    }

    #[test]
    fn begin_frame_drops_stale_inputs() {
        let mut gate = ScenePickGate::default();
        let body = rect((0.0, 30.0), (200.0, 400.0));
        gate.begin_frame();
        gate.mark_rendered();
        gate.record_chrome_panel(body, body);
        gate.set_dock_rect(body);
        gate.resolve(hovering((100.0, 100.0)));
        assert_eq!(gate.resolved(), None);

        gate.begin_frame();
        gate.mark_rendered();
        gate.resolve(hovering((100.0, 100.0)));
        assert_eq!(gate.resolved(), Some(SceneTarget::MainViewport));
    }
}
