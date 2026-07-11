//! UI for the sandbox editing tools.
//!
//! All UI lives here. Panels are pure presentation — they query state
//! and emit commands. They never mutate domain state directly (except for
//! UI-local state like SpawnState and SelectedEntity).

use bevy::prelude::*;
use lunco_workbench::{
    HelpMouse, HelpShortcut, PanelId, ViewportPanel, Perspective, PerspectiveId, WorkbenchAppExt,
    WorkbenchLayout, VIEWPORT_PANEL_ID,
};

pub mod spawn_palette;
pub mod inspector;
pub mod entity_list;
pub mod terrain_tools;
pub mod connection_canvas;
pub mod usd_prim_tree;
pub mod usd_params;

/// Schedule slot (in `Update`) for the UI *view-model* producers — the
/// change-driven systems that derive render-ready state into resources for the
/// egui panels to read (WP-8). `Update` runs before `EguiPrimaryContextPass`, so
/// resources written here are visible to the panels the same frame. Later panels
/// add their producers to this set; gate each with its own `run_if` so it only
/// runs when its source data changes.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ViewModelSet;

/// Plugin that registers all sandbox editing UI panels, the workbench
/// 3D viewport placeholder, and two workspace presets:
///
/// - **View** (default) — just the 3D scene, no panels.
/// - **Build** — 3D + Entities, Inspector, Spawn palette around the edges.
///
/// The user switches via the workspace tabs in the transport bar.
/// `ViewportPanel` reserves the centre slot in both perspectives; the
/// 3D camera (tagged `WorkbenchViewportCamera`) is confined to that
/// rect each frame by `lunco_workbench::apply_workbench_viewport`, and
/// the panel paints its theme backdrop around it.
pub struct SandboxEditUiPlugin;

impl Plugin for SandboxEditUiPlugin {
    fn build(&self, app: &mut App) {
        app.register_panel(spawn_palette::SpawnPalette)
            .register_panel(inspector::Inspector)
            .register_panel(entity_list::EntityList)
            .register_panel(terrain_tools::ToolsPanel)
            .register_panel(connection_canvas::UsdCanvasPanel)
            .register_panel(usd_prim_tree::UsdPrimTreePanel)
            .register_panel(ViewportPanel)
            // Order matters for auto-activation — View first so it's
            // the default when the rover binary boots.
            .register_perspective(ViewPerspective)
            .register_perspective_help(
                PerspectiveId("sandbox_view"),
                lunco_workbench::PerspectiveHelp {
                    title: "🎬 View",
                    description: "Full-screen 3D observation & control mode. Fly the \
                                  camera around the scene, take control of a vessel \
                                  and drive it, or follow one as it moves.",
                    shortcuts: vec![
                        HelpShortcut { keys: "W / A / S / D", description: "Drive the controlled vessel · fly the camera" },
                        HelpShortcut { keys: "Q / E", description: "Move camera down / up" },
                        HelpShortcut { keys: "Shift", description: "Camera speed boost" },
                        HelpShortcut { keys: "Space", description: "Brake the controlled vessel" },
                        HelpShortcut { keys: "Backspace", description: "Release control — back to free-flight camera" },
                        HelpShortcut { keys: "Esc", description: "Drop the transform gizmo / deselect" },
                        HelpShortcut { keys: "+ / −", description: "Zoom in / out" },
                    ],
                    mouse: vec![
                        HelpMouse { interaction: "Left-Click vessel", description: "Take control (possess) and drive it" },
                        HelpMouse { interaction: "Left-Click object", description: "Follow it with the camera" },
                        HelpMouse { interaction: "Alt+Left-Click", description: "Grab the transform gizmo to move an object" },
                        HelpMouse { interaction: "Right-Drag", description: "Orbit / rotate the camera" },
                        HelpMouse { interaction: "Scroll", description: "Zoom in / out" },
                    ],
                    has_tour: false,
                },
            )
            .register_perspective(BuildPerspective)
            .register_perspective_help(
                PerspectiveId("rover_build"),
                lunco_workbench::PerspectiveHelp {
                    title: "🏗 Build",
                    description: "3D scene editor. Spawn objects from the palette, \
                                  select and transform them, and assemble the scene.",
                    shortcuts: vec![
                        HelpShortcut { keys: "W / A / S / D", description: "Move camera" },
                        HelpShortcut { keys: "Q / E", description: "Move camera down / up" },
                        HelpShortcut { keys: "Shift", description: "Hold to place multiple (sticky spawn)" },
                        HelpShortcut { keys: "Delete", description: "Delete the selected object" },
                        HelpShortcut { keys: "Ctrl+Z", description: "Undo" },
                        HelpShortcut { keys: "Esc", description: "Cancel placement · clear selection / gizmo" },
                    ],
                    mouse: vec![
                        HelpMouse { interaction: "Left-Click", description: "Select object · confirm placement" },
                        HelpMouse { interaction: "Alt+Left-Click", description: "Select + transform gizmo (drag to move)" },
                        HelpMouse { interaction: "Right-Drag", description: "Orbit / rotate the camera" },
                        HelpMouse { interaction: "Scroll", description: "Zoom in / out" },
                    ],
                    has_tour: false,
                },
            )
            .register_perspective(TerrainPerspective)
            .register_perspective_help(
                PerspectiveId("terrain_sculpt"),
                lunco_workbench::PerspectiveHelp {
                    title: "🏔 Terrain",
                    description: "Sculpt the surface. Arm a brush in the Tools palette, \
                                  then click the terrain to raise, dig, or flatten it. \
                                  Edits re-bake the visuals and the collider live.",
                    shortcuts: vec![
                        HelpShortcut { keys: "Shift + ↑/↓", description: "Grow / shrink brush radius" },
                        HelpShortcut { keys: "Alt + ↑/↓", description: "Grow / shrink brush strength" },
                        HelpShortcut { keys: "Esc", description: "Disarm the brush" },
                    ],
                    mouse: vec![
                        HelpMouse { interaction: "Left-Click", description: "Sculpt (raise) · flatten to clicked height" },
                        HelpMouse { interaction: "Alt+Left-Click", description: "Dig (invert the sculpt)" },
                        HelpMouse { interaction: "Ctrl+Left-Click", description: "Flatten to the clicked height" },
                        HelpMouse { interaction: "Shift / Alt + Scroll", description: "Brush radius / strength" },
                        HelpMouse { interaction: "Right-Drag", description: "Orbit / rotate the camera" },
                    ],
                    has_tour: false,
                },
            )
            .register_perspective(ObjectBuilderPerspective)
            .register_perspective_help(
                PerspectiveId("object_builder"),
                lunco_workbench::PerspectiveHelp {
                    title: "🧩 Object Builder",
                    description: "Assemble and edit objects from parts. Navigate the \
                                  object's structure in the tree, attach components from \
                                  the palette, and tune the selected prim's parameters in \
                                  the Inspector.",
                    shortcuts: vec![
                        HelpShortcut { keys: "Ctrl+Z", description: "Undo the last edit" },
                        HelpShortcut { keys: "Delete", description: "Remove the selected part" },
                        HelpShortcut { keys: "Esc", description: "Clear selection / gizmo" },
                    ],
                    mouse: vec![
                        HelpMouse { interaction: "Click a tree node", description: "Select a part to inspect / edit" },
                        HelpMouse { interaction: "Alt+Left-Click", description: "Select + transform gizmo (drag to move)" },
                        HelpMouse { interaction: "Right-Drag", description: "Orbit / rotate the camera" },
                    ],
                    has_tour: false,
                },
            );

        // WP-8: the Entity list is a pure view over `EntityTreeView`, derived by
        // a change-gated producer instead of being rebuilt every egui frame.
        app.init_resource::<entity_list::EntityTreeView>().add_systems(
            Update,
            entity_list::populate_entity_tree_view
                .in_set(ViewModelSet)
                .run_if(entity_list::scene_topology_changed),
        );

        // WP-8: the Inspector reads query-derived sun / camera / joint state
        // (which `PanelCtx` can't gather in paint) from `InspectorView`,
        // produced each frame by an exclusive system before the egui pass.
        app.init_resource::<inspector::InspectorView>().add_systems(
            Update,
            inspector::populate_inspector_view
                .in_set(ViewModelSet)
                .run_if(inspector::inspector_inputs_changed),
        );

        // USD connection canvas: the scene is derived from the live composed
        // stage by a main-thread producer (the stage is `!Send`), hash-gated so
        // it only rebuilds on a topology change. No `run_if` — the system
        // early-returns cheaply when nothing is wired or the topology is stable.
        app.init_resource::<connection_canvas::UsdCanvasState>().add_systems(
            Update,
            connection_canvas::produce_usd_canvas.in_set(ViewModelSet),
        );

        // USD prim tree: same main-thread producer pattern (the stage is
        // `!Send`), hash-gated on the prim-path set.
        app.init_resource::<usd_prim_tree::UsdPrimTreeView>().add_systems(
            Update,
            usd_prim_tree::produce_usd_prim_tree.in_set(ViewModelSet),
        );

        // USD parameter sliders: harvest the selected prim's customData-ranged
        // attributes for the Inspector's data-driven Parameters section.
        app.init_resource::<usd_params::UsdParamView>().add_systems(
            Update,
            usd_params::produce_usd_param_view.in_set(ViewModelSet),
        );

        // Debug-viz settings menu rows (joint + wheel-force gizmos).
        app.add_systems(Startup, register_debug_viz_settings);
    }
}

/// Register checkbox rows in the workbench Settings menu for the joint
/// and wheel-force gizmos. Mutates [`joint_viz::JointVizSettings`]
/// directly; the resource is not persisted (debug toggle, defaults off).
fn register_debug_viz_settings(world: &mut World) {
    use bevy_egui::egui;
    let Some(mut layout) = world.get_resource_mut::<WorkbenchLayout>() else {
        return;
    };
    layout.register_settings(|ui, world| {
        ui.label(egui::RichText::new("Debug Visualization").weak().small());
        let mut settings = world.resource_mut::<crate::joint_viz::JointVizSettings>();
        ui.checkbox(&mut settings.show_joints, "Show joints")
            .on_hover_text("Draw anchor dots + axis lines for every Avian joint");
        ui.checkbox(&mut settings.show_wheel_forces, "Show wheel forces")
            .on_hover_text("Draw a force box + arrow at every wheel");
    });
}

/// Rover sandbox's default workspace — full-screen 3D, no panels.
///
/// All slots empty — the workbench renders **nothing** in the
/// centre, so Bevy's 3D scene gets the pointer events directly. This
/// is the only way to keep gizmos draggable without render-to-texture:
/// any egui surface in the central area (even a transparent
/// `ViewportPanel` tab) marks the rect as egui-interactive and
/// blocks Bevy input.
pub struct ViewPerspective;

impl Perspective for ViewPerspective {
    fn id(&self) -> PerspectiveId { PerspectiveId("sandbox_view") }
    fn title(&self) -> String { "🎬 View".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser(None);
        layout.set_right_inspector(None);
        layout.set_bottom(None);
        layout.set_center(vec![]);
    }
}

/// Build mode — Spawn left, 3D centre, Inspector + Entities tabbed right.
///
/// Spawn lives on the left because it's the primary "add stuff" tool;
/// Entities + Inspector tab together on the right because the entity
/// list is rarely the main view (you mostly click in the 3D scene to
/// select). Bottom dock is empty — fewer rows of chrome.
pub struct BuildPerspective;

impl Perspective for BuildPerspective {
    fn id(&self) -> PerspectiveId { PerspectiveId("rover_build") }
    // ⚒ (U+2692) instead of 🏗 (U+1F3D7) — the latter tofus in the
    // bundled DejaVu fallback; ⚒ renders everywhere (see welcome.rs).
    fn title(&self) -> String { "⚒ Build".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser_tabs(vec![
            PanelId("spawn_palette"),
            PanelId("tools_palette"),
            // Optional — registered by the rover binary; filtered out
            // in other apps.
            PanelId("rover_models"),
        ]);
        layout.set_center(vec![VIEWPORT_PANEL_ID]);
        layout.set_right_inspector_tabs(vec![
            PanelId("sandbox_inspector"),
            PanelId("entity_list"),
            // Optional — only renders if the host binary registers a
            // panel with this id (the rover binary does, modelica
            // workbench doesn't). The workbench filters unknown ids.
            PanelId("rover_code"),
        ]);
        layout.set_bottom(None);
    }
}

/// Object Builder mode — assemble and edit objects from parts.
///
/// Distinct from Build (which leads with the spawn palette for dropping loose
/// props into a scene): this leads with the **object's structure** — the entity
/// tree on the left, so you navigate and select a rover's rocker → bogie → wheel
/// — with the component palette beneath it for attaching parts, the 3D view in the
/// centre, and the Inspector on the right to tune the selected prim's parameters.
/// The panels are the proven ones (tree / palette / viewport / inspector); this is
/// the workspace that arranges them for building rather than observing.
///
/// The connection canvas and rhai editor that will also live here are separate,
/// larger additions; this establishes the perspective they dock into.
pub struct ObjectBuilderPerspective;

impl Perspective for ObjectBuilderPerspective {
    fn id(&self) -> PerspectiveId { PerspectiveId("object_builder") }
    // 🧩 renders in the bundled fallback (unlike 🏗, which tofus — see welcome.rs).
    fn title(&self) -> String { "🧩 Object Builder".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        // Structure first: the USD prim tree (the object's authoring hierarchy)
        // to navigate/select parts, the entity list as an alternate view, and the
        // palette to add parts. (Unknown ids are filtered.)
        layout.set_side_browser_tabs(vec![
            usd_prim_tree::USD_PRIM_TREE_PANEL_ID,
            PanelId("entity_list"),
            PanelId("spawn_palette"),
        ]);
        // Three central tabs: the 3D build view, the connection canvas, and the
        // Rhai behaviour editor. The canvas rewires co-sim connections and joints;
        // the editor edits the selected prim's script; the 3D view places and
        // transforms parts. Viewport first so it's the default tab (its 3D renders
        // through the empty tab). `rhai_editor` is registered by the sandbox binary
        // (the workbench filters the id in apps that don't register it).
        layout.set_center(vec![
            VIEWPORT_PANEL_ID,
            connection_canvas::USD_CANVAS_PANEL_ID,
            PanelId("rhai_editor"),
        ]);
        // The Inspector alone on the right — parameter editing is the point here.
        layout.set_right_inspector_tabs(vec![PanelId("sandbox_inspector")]);
        layout.set_bottom(None);
    }
}

/// Terrain sculpt mode — Tools palette left, 3D centre, Inspector + Entities
/// tabbed right. The Tools palette arms a brush; clicking the terrain sculpts
/// it (possession + selection stand down while a brush is armed).
pub struct TerrainPerspective;

impl Perspective for TerrainPerspective {
    fn id(&self) -> PerspectiveId { PerspectiveId("terrain_sculpt") }
    fn title(&self) -> String { "🏔 Terrain".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser_tabs(vec![PanelId("tools_palette")]);
        layout.set_center(vec![VIEWPORT_PANEL_ID]);
        layout.set_right_inspector_tabs(vec![
            PanelId("sandbox_inspector"),
            PanelId("entity_list"),
        ]);
        layout.set_bottom(None);
    }
}
