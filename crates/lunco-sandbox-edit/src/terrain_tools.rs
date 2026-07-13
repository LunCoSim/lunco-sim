//! Terrain-sculpt tools — click-to-edit with a brush ghost preview.
//!
//! The Tools palette ([`ui::terrain_tools`]) arms a [`TerrainTool`]; while one
//! is armed every scene left-click applies a terrain edit at the cursor instead
//! of possessing / selecting. The edit is emitted as the existing
//! [`BrushTerrain`] / [`FlattenTerrain`] command, so it flows through the one
//! authoring path (document-free → direct stack edit; document-backed → USD
//! authoring on the runtime layer) and re-bakes visuals + collider — identical
//! to the rhai `terrain::dig` / HTTP / MCP route.
//!
//! Modifiers vary the click; keyboard arrows / scroll size the brush:
//!
//! | Input                     | Effect                                          |
//! |---------------------------|-------------------------------------------------|
//! | Left-click (Sculpt)       | raise a berm by `strength`                      |
//! | Alt + Left-click (Sculpt) | invert — dig a pit                              |
//! | Ctrl + Left-click         | flatten to the clicked height (quick pad)       |
//! | Left-click (Flatten)      | flatten to the clicked height                   |
//! | Shift + ↑/↓  · Shift+scroll| grow / shrink brush **radius**                  |
//! | Alt + ↑/↓    · Alt+scroll  | grow / shrink brush **strength**                |
//! | Esc                       | disarm the tool                                 |

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use big_space::prelude::Grid;
use lunco_terrain_surface::{BrushTerrain, FlattenTerrain, PlaceCrater, PlaceRock};

/// Which terrain brush is armed. `None` = the tool is off and clicks pass
/// through to possess / select as usual.
#[derive(Default, PartialEq, Eq, Clone, Copy, Debug)]
pub enum TerrainTool {
    /// No tool armed.
    #[default]
    None,
    /// Raise (or, with Alt, lower) the surface under the cursor.
    Sculpt,
    /// Level the surface toward the clicked height — the landing-pad tool.
    Flatten,
    /// Stamp one realistic impact crater (rim radius = brush radius) at the
    /// clicked point. Same analytic morphology as the procedural field.
    Crater,
    /// Place one boulder (radius = brush radius, capped) at the clicked point.
    Rock,
}

/// Live terrain-tool state, driven by the Tools palette and the scene click /
/// keyboard handlers. `radius` (metres) and `strength` (metres of height change
/// per click) are shared by every brush.
#[derive(Resource)]
pub struct TerrainToolState {
    /// The armed brush, or [`TerrainTool::None`].
    pub tool: TerrainTool,
    /// Brush radius in metres.
    pub radius: f32,
    /// Height delta applied per click, in metres (Sculpt only).
    pub strength: f32,
}

impl Default for TerrainToolState {
    fn default() -> Self {
        Self { tool: TerrainTool::None, radius: 5.0, strength: 0.5 }
    }
}

impl TerrainToolState {
    /// Whether a brush is currently armed.
    pub fn armed(&self) -> bool {
        self.tool != TerrainTool::None
    }
}

const RADIUS_MIN: f32 = 0.5;
const RADIUS_MAX: f32 = 200.0;
const STRENGTH_MIN: f32 = 0.05;
const STRENGTH_MAX: f32 = 50.0;

/// Ghost ring shown at the cursor while a brush is armed — a translucent unit
/// disc scaled to `radius`, tinted by the action (green raise / red dig /
/// blue flatten).
#[derive(Component)]
pub struct TerrainBrushGhost;

/// Mirror the armed state into the shared [`lunco_core::TerrainToolActive`] gate
/// (read by possession + selection so they stand down while sculpting) and
/// disarm on Escape. Keyboard-driven, so it stays a plain system.
pub fn terrain_tool_state_system(
    mut state: ResMut<TerrainToolState>,
    mut active: ResMut<lunco_core::TerrainToolActive>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    active.0 = state.armed();
    if state.armed() && keys.just_pressed(KeyCode::Escape) {
        state.tool = TerrainTool::None;
    }
}

/// Size the brush from the keyboard (Shift/Alt + ↑/↓) and the scroll wheel
/// (Shift/Alt + scroll — the "two-finger gesture"). Only runs while armed, so
/// it never steals the wheel from camera zoom in normal navigation.
pub fn terrain_brush_size_input(
    mut state: ResMut<TerrainToolState>,
    keys: Res<ButtonInput<KeyCode>>,
    mut wheel: MessageReader<MouseWheel>,
) {
    if !state.armed() {
        wheel.clear();
        return;
    }
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);

    // Discrete arrow steps.
    let mut up = keys.just_pressed(KeyCode::ArrowUp) as i32 as f32;
    up -= keys.just_pressed(KeyCode::ArrowDown) as i32 as f32;

    // Continuous scroll (line- and pixel-scroll both report on `.y`).
    let mut scroll = 0.0;
    for ev in wheel.read() {
        scroll += ev.y.signum();
    }

    if shift {
        // Radius: 1 m per arrow step, 2 m per scroll notch.
        let d = up * 1.0 + scroll * 2.0;
        if d != 0.0 {
            state.radius = (state.radius + d).clamp(RADIUS_MIN, RADIUS_MAX);
        }
    } else if alt {
        // Strength: 0.1 m per arrow step / scroll notch.
        let d = (up + scroll) * 0.1;
        if d != 0.0 {
            state.strength = (state.strength + d).clamp(STRENGTH_MIN, STRENGTH_MAX);
        }
    }
}

/// Colour the brush action would produce, given live modifiers — reused by the
/// ghost tint and (implicitly) the palette hint.
fn action_color(tool: TerrainTool, alt: bool, ctrl: bool) -> Color {
    match tool {
        TerrainTool::Flatten => Color::srgba(0.4, 0.6, 1.0, 0.35), // blue
        TerrainTool::Sculpt if ctrl => Color::srgba(0.4, 0.6, 1.0, 0.35), // blue (quick flatten)
        TerrainTool::Sculpt if alt => Color::srgba(1.0, 0.4, 0.4, 0.35), // red (dig)
        TerrainTool::Sculpt => Color::srgba(0.4, 1.0, 0.5, 0.35),  // green (raise)
        TerrainTool::Crater => Color::srgba(1.0, 0.75, 0.3, 0.35), // orange (impact)
        TerrainTool::Rock => Color::srgba(0.75, 0.7, 0.6, 0.35),   // grey-tan (boulder)
        TerrainTool::None => Color::NONE,
    }
}

/// Follow the cursor with a translucent brush-footprint disc while armed. Casts
/// a ray through the active window camera onto the terrain **height oracle**
/// (same approach as the spawn ghost — the collider ring only exists near
/// dynamic bodies, so a collider cast floats or misses over open ground),
/// falling back to physics for non-DEM scenes, and scales the unit disc to
/// `radius`.
pub fn update_terrain_brush_ghost(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    state: Res<TerrainToolState>,
    keys: Res<ButtonInput<KeyCode>>,
    cameras: Query<(&Camera, &GlobalTransform, &bevy::camera::RenderTarget), With<Camera3d>>,
    windows: Query<&Window>,
    mut q_ghost: Query<
        (Entity, &mut Transform, &mut lunco_render::PbrLook),
        With<TerrainBrushGhost>,
    >,
    grids: Query<Entity, With<Grid>>,
    raycaster: avian3d::prelude::SpatialQuery,
    terrains: crate::spawn::TerrainOracles,
) {
    if !state.armed() {
        for (ghost, _, _) in q_ghost.iter() {
            commands.entity(ghost).despawn();
        }
        return;
    }

    // Ray through the ACTIVE window camera (not merely the first Camera3d).
    let Some((camera, cam_tf)) = cameras
        .iter()
        .find(|(cam, _, target)| {
            cam.is_active && matches!(target, bevy::camera::RenderTarget::Window(_))
        })
        .map(|(cam, tf, _)| (cam, tf))
    else {
        return;
    };
    let Some(window) = windows.iter().next() else { return };
    let Some(cursor) = window.cursor_position() else { return };
    let Ok(ray) = camera.viewport_to_world(cam_tf, cursor) else { return };
    let origin = ray.origin.as_dvec3();
    let dir = ray.direction;

    // The brush edits the terrain, so the oracle hit IS the target surface;
    // physics is only the fallback for scenes without a DEM terrain.
    let Some(point) = crate::spawn::terrain_ray_hit(&terrains, origin, dir.as_dvec3(), 10_000.0)
        .map(|(_, p)| p)
        .or_else(|| {
            raycaster
                .cast_ray(
                    origin,
                    dir,
                    10_000.0,
                    false,
                    &avian3d::prelude::SpatialQueryFilter::default(),
                )
                .map(|h| origin + dir.as_dvec3() * h.distance)
        })
        .map(|p| p.as_vec3())
    else {
        return;
    };

    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let color = action_color(state.tool, alt, ctrl);

    // Lift the disc a hair off the surface so it doesn't z-fight the terrain.
    let transform = Transform::from_translation(point + Vec3::Y * 0.05)
        .with_scale(Vec3::new(state.radius, 1.0, state.radius));

    if let Some((_, mut tf, mut look)) = q_ghost.iter_mut().next() {
        // `set_if_neq`, NOT `*tf = transform`: the latter goes through `DerefMut`, so
        // it marks `Changed<Transform>` EVERY frame the brush is armed — whether or
        // not the cursor moved (and every downstream propagation with it). Compare
        // first, write only on a real move.
        tf.set_if_neq(transform);
        // Same discipline for the tint: `DerefMut` on `PbrLook` marks it `Changed`,
        // and the render binder re-materialises on `Changed<PbrLook>` — so an
        // unconditional write would re-bind every frame. The tint only changes when
        // a modifier (Alt/Ctrl) toggles: compare first, write only on a real change.
        if look.base_color != color.to_linear() {
            look.base_color = color.to_linear();
        }
    } else {
        let Some(grid) = grids.iter().next() else { return };
        // Unit-radius flat disc; scaled by `radius` each frame via the transform.
        let mesh = meshes.add(Cylinder::new(1.0, 0.02).mesh().resolution(48).build());
        // `Visibility` pulls in `InheritedVisibility` + `ViewVisibility` as required
        // components in Bevy 0.19 — no need to insert them explicitly.
        commands.spawn((
            Name::new("TerrainBrushGhost"),
            TerrainBrushGhost,
            transform,
            Mesh3d(mesh),
            // Appearance INTENT — the render binder makes the material. Only a
            // handful of distinct tints exist (one per tool × modifier), so the
            // binder's look-keyed cache holds a handful of materials, not one
            // per frame.
            lunco_render::PbrLook {
                base_color: color.to_linear(),
                unlit: true,
                alpha: lunco_render::SurfaceAlpha::Blend,
                perceptual_roughness: 0.5,
                ..default()
            },
            ChildOf(grid),
            Visibility::Visible,
        ));
    }
}

/// Apply a terrain edit where the user clicks, driven by **bevy_picking**.
///
/// Registered as a global `On<Pointer<Click>>` observer. Only acts while a
/// brush is armed; possession + selection stand down via
/// [`lunco_core::TerrainToolActive`], so the click is ours. `hit.position` is
/// the world point on the terrain mesh — no manual ray-cast. Emits the same
/// [`BrushTerrain`] / [`FlattenTerrain`] command the scripting / API paths use.
pub fn on_scene_click_terrain(
    mut click: On<bevy::picking::events::Pointer<bevy::picking::events::Click>>,
    state: Res<TerrainToolState>,
    // Gate on the SAME resource selection/possession read (not `state.armed()`
    // directly): `terrain_tool_state_system` mirrors `state` into `TerrainToolActive`
    // one system later, so reading `state` here would open a one-frame window where
    // this observer thinks the tool is armed while selection still thinks it is not
    // (or vice-versa) — a click that both sculpts AND selects/possesses. One gate,
    // one source of truth.
    active: Res<lunco_core::TerrainToolActive>,
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
) {
    use bevy::picking::pointer::PointerButton;
    if !active.0 {
        return;
    }
    // `active` says a tool is armed; the disarm mirror lags a frame, so if `state`
    // has already cleared the tool this frame, drop the click (don't sculpt with
    // `TerrainTool::None`) rather than fall through to a default brush.
    if state.tool == TerrainTool::None {
        return;
    }
    // We own the click while armed — stop it bubbling to ancestors.
    click.propagate(false);
    if click.button != PointerButton::Primary {
        return;
    }
    // Chrome guard — egui's pick carries no world position; a terrain hit does.
    let Some(point) = click.hit.position else {
        return;
    };

    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let (x, z, radius) = (point.x, point.z, state.radius);

    // Crater stamps one impact; Rock drops one boulder; Ctrl overrides Sculpt
    // into a one-shot flatten-to-clicked-height; the Flatten tool always flattens.
    if state.tool == TerrainTool::Crater {
        // depth 0 → the command's realistic default (0.4·radius).
        commands.trigger(PlaceCrater { x, z, radius, depth: 0.0, id: String::new() });
    } else if state.tool == TerrainTool::Rock {
        // size 0 would mean "default"; the brush radius is the boulder radius
        // (the command clamps it to sane boulder bounds). seed 0 = derived.
        commands.trigger(PlaceRock { x, z, size: radius, seed: 0, id: String::new() });
    } else if state.tool == TerrainTool::Flatten || ctrl {
        commands.trigger(FlattenTerrain { x, z, radius, target_y: point.y, id: String::new() });
    } else {
        let amplitude = if alt { -state.strength } else { state.strength };
        commands.trigger(BrushTerrain { x, z, radius, amplitude, id: String::new() });
    }
}
