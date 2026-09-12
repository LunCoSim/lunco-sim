//! Draws the labels prims asked for via `lunco:billboard*`
//! ([`UsdBillboard`](lunco_usd_sim::billboard::UsdBillboard)).
//!
//! Screen space is the right space for this. A world-space text mesh would have
//! to be re-oriented every frame, would scale itself into illegibility, and
//! would z-fight the terrain it labels; an egui overlay is always camera-facing
//! and always crisp. The label's content and owner still come from the USD prim;
//! this pass only projects the existing render pose into the viewport.
//!
//! ## Three things this must get right
//!
//! **big_space.** Projection uses the subject's and camera's
//! [`GlobalTransform`] values from the same render frame. Those are the exact
//! camera-relative poses used by the mesh renderer, so a label cannot combine
//! an interpolated render pose with an independently sampled grid pose. The
//! Geodetic text (`{lat}`, `{lon}`, `{height}`) comes from
//! [`lunco_celestial::SurfacePoseQuery`], which resolves the entity in the
//! explicit body-fixed frame. Root-world coordinates are never interpreted as
//! site ENU.
//!
//! **Depth.** egui paints over everything, so a label whose subject is behind a
//! ridge would otherwise still be readable. Labels are drawn nearest-last, and
//! each is dropped once its subject passes `fade_end`. True occlusion would
//! need a depth read this overlay does not have; the honest mitigation is the
//! distance cut plus a backdrop chip so text never dissolves into terrain. The
//! layout pass gives nearer labels first choice of a bounded screen-space slot;
//! labels that cannot be placed without colliding are omitted rather than
//! painted on top of one another.
//!
//! **Viewport ownership.** The scene camera renders the window as a layered
//! surface and egui draws the workbench chrome over it. A screen-space label is
//! therefore clipped to the measured `ViewportPanel` rect and appended to the
//! workbench's root background paint list. The latter is important: an ad-hoc
//! `Order::Background` layer has no deterministic order relative to the root
//! layer in egui, while appending before `WorkbenchRenderSet` gives the real
//! 3D → tags → UI sequence.

use bevy::prelude::*;
use bevy_egui::egui;
use big_space::prelude::{CellCoord, Grid};
use lunco_core::coords::world_vector;
use lunco_render::SceneCamera;
use lunco_usd_sim::billboard::{render_billboard, BillboardFacts, BillboardIndex, UsdBillboard};
use lunco_workbench::{PanelRects, VIEWPORT_PANEL_ID};

const BILLBOARD_FONT_SIZE: f32 = 13.0;
const BILLBOARD_MAX_WIDTH: f32 = 220.0;
const BILLBOARD_LABEL_GAP: f32 = 12.0;
const BILLBOARD_BACKDROP_PADDING: egui::Vec2 = egui::vec2(5.0, 3.0);
const BILLBOARD_VIEWPORT_MARGIN: f32 = 6.0;
const BILLBOARD_ANCHOR_GUARD: egui::Vec2 = egui::vec2(12.0, 12.0);

/// Resolve the screen-space anchor from the same render pose as the subject's
/// mesh. Keeping this small and explicit prevents callers from accidentally
/// substituting an authoritative grid pose during visual projection.
fn render_anchor(gtf: &GlobalTransform, offset_y: f32) -> Vec3 {
    gtf.translation() + Vec3::Y * offset_y
}

/// Paint every visible [`UsdBillboard`].
#[allow(clippy::too_many_arguments)]
pub fn draw_billboard_overlay(
    // `Callsign` is `ui:displayName` — the standard USD human name, ingested in
    // `lunco-usd-bevy`. It is what `{label}` means: the ground stations author
    // "Bear Lakes · RT-64" there, and without this the token fell back to the prim
    // NAME ("BearLakes") on every label in every scene, because this system passed
    // `label: None` unconditionally and nothing else ever filled it.
    q_billboards: Query<(
        Entity,
        &UsdBillboard,
        &Name,
        Option<&BillboardIndex>,
        Option<&ViewVisibility>,
        Option<&lunco_core::markers::Callsign>,
        Option<&lunco_core::CatalogEntryId>,
        &GlobalTransform,
    )>,
    q_camera: Query<(&Camera, &GlobalTransform), (With<Camera3d>, With<SceneCamera>)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    surface_pose: lunco_celestial::SurfacePoseQuery,
    scene_viewport: Res<lunco_core::SceneViewport>,
    panel_rects: Option<Res<PanelRects>>,
    mut egui_ctx: bevy_egui::EguiContexts,
    theme: Option<Res<lunco_theme::Theme>>,
) {
    if q_billboards.is_empty() {
        return;
    }
    // A hidden workbench scene must not leave one frame of screen-space labels
    // behind while the camera reconciler turns its window camera off.
    if !scene_viewport.visible {
        return;
    }
    let Some(camera_entity) = scene_viewport.active_camera else {
        return;
    };
    let Ok((camera, cam_gtf)) = q_camera.get(camera_entity) else {
        return;
    };
    if !camera.is_active {
        return;
    }
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let origin = ctx.content_rect().min.to_vec2();
    let clip_rect = panel_rects
        .as_ref()
        .and_then(|rects| rects.egui_rect(VIEWPORT_PANEL_ID, ctx))
        .unwrap_or_else(|| ctx.content_rect());
    let theme = theme
        .map(|t| t.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);

    // Use the root background paint list, not a second custom Background layer.
    // egui does not guarantee an order between ad-hoc layers that are absent
    // from its Area order map; the system itself is scheduled before the
    // workbench, so appending here gives a deterministic 3D → tag → UI stack.
    let painter = ctx
        .layer_painter(egui::LayerId::background())
        .with_clip_rect(clip_rect);

    // Collect first so the layout pass can give the nearest labels first choice
    // of a slot, then paint far-to-near: with no depth buffer, drawing nearest
    // LAST is what keeps a close label on top of a distant one.
    struct Drawn {
        entity: Entity,
        screen: egui::Pos2,
        text: String,
        distance: f64,
        fade_end: f32,
    }
    let mut drawn: Vec<Drawn> = Vec::new();

    for (entity, bb, name, billboard_index, vis, callsign, catalog_id, gtf) in &q_billboards {
        // An entity culled or explicitly hidden must not keep a floating label.
        if vis.is_some_and(|v| !v.get()) {
            continue;
        }
        // The render pose is the billboard's own GlobalTransform, not a
        // second reconstruction from `(CellCoord, Transform)`. Avian/big_space
        // may ease/rebase those authoritative components between simulation
        // ticks; reconstructing them here while projecting through the
        // camera's already-propagated GlobalTransform mixes two pose phases
        // and makes a label jitter against the body it annotates.
        let anchor_render = render_anchor(gtf, bb.offset_y);
        let Some(distance) = world_vector(camera_entity, entity, &q_parents, &q_grids, &q_spatial)
            .map(|vector| vector.length())
        else {
            continue;
        };
        if distance > bb.fade_end as f64 {
            continue;
        }

        // GlobalTransform is already in the floating-origin-relative render
        // frame. Passing it directly preserves the same pose the rover mesh
        // uses and avoids an absolute f64 -> f32 round trip for the visual
        // anchor.
        let Ok(viewport) = camera.world_to_viewport(cam_gtf, anchor_render) else {
            continue; // behind the camera
        };

        let display_name = lunco_core::entity_display_name(Some(name), callsign, catalog_id);
        let geo = surface_pose.get(entity).map(|pose| pose.geodetic);
        let text = render_billboard(
            &bb.template,
            &BillboardFacts {
                name: &display_name,
                label: callsign.map(|c| c.0.as_str()),
                index: billboard_index.map(|index| index.0),
                geo,
            },
        );
        drawn.push(Drawn {
            entity,
            screen: egui::pos2(viewport.x, viewport.y) + origin,
            text,
            distance,
            fade_end: bb.fade_end,
        });
    }

    drawn.sort_by(|a, b| {
        a.distance
            .total_cmp(&b.distance)
            .then_with(|| a.entity.to_bits().cmp(&b.entity.to_bits()))
    });

    let wrap_width = (clip_rect.width()
        - 2.0 * (BILLBOARD_VIEWPORT_MARGIN + BILLBOARD_BACKDROP_PADDING.x))
        .clamp(1.0, BILLBOARD_MAX_WIDTH);
    let safe_clip = clip_rect.shrink(BILLBOARD_VIEWPORT_MARGIN + BILLBOARD_BACKDROP_PADDING.x);
    let mut placed = Vec::with_capacity(drawn.len());
    let mut occupied = Vec::with_capacity(drawn.len());

    for d in drawn {
        // Fade with distance so far labels recede instead of all shouting
        // equally; never fully transparent before `fade_end` drops it outright.
        let fade = (1.0 - (d.distance as f32 / d.fade_end)).clamp(0.25, 1.0);
        let alpha = (255.0 * fade) as u8;
        let c = theme.tokens.text;
        let color = egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha);
        let galley = painter.layout(
            d.text,
            egui::FontId::proportional(BILLBOARD_FONT_SIZE),
            color,
            wrap_width,
        );
        let Some(top_left) = place_label(d.screen, galley.size(), safe_clip, &occupied) else {
            continue;
        };
        let bg =
            egui::Rect::from_min_size(top_left, galley.size()).expand2(BILLBOARD_BACKDROP_PADDING);
        occupied.push(bg);
        placed.push(Placed {
            top_left,
            galley,
            color,
            entity: d.entity,
            distance: d.distance,
            fade,
            bg,
        });
    }

    // Keep the established depth ordering for the labels that survived layout.
    placed.sort_by(|a, b| {
        b.distance
            .total_cmp(&a.distance)
            .then_with(|| b.entity.to_bits().cmp(&a.entity.to_bits()))
    });

    for label in placed {
        let backdrop = theme.tokens.overlay_backdrop;
        painter.rect_filled(
            label.bg,
            3.0,
            egui::Color32::from_rgba_unmultiplied(
                backdrop.r(),
                backdrop.g(),
                backdrop.b(),
                (f32::from(backdrop.a()) * label.fade) as u8,
            ),
        );
        painter.galley(label.top_left, label.galley, label.color);
    }
}

struct Placed {
    entity: Entity,
    top_left: egui::Pos2,
    galley: std::sync::Arc<egui::Galley>,
    color: egui::Color32,
    distance: f64,
    fade: f32,
    bg: egui::Rect,
}

/// Pick a bounded, non-overlapping screen-space slot for one label.
///
/// Nearer labels are passed first. The preferred slot is above the marker;
/// alternate sides let a dense route keep several labels legible without
/// drawing one chip over another. The anchor guard keeps a chip from covering
/// the marker it describes, and `safe_clip` keeps the backdrop inside the
/// viewport rather than merely clipping the text after it has been placed.
fn place_label(
    anchor: egui::Pos2,
    size: egui::Vec2,
    safe_clip: egui::Rect,
    occupied: &[egui::Rect],
) -> Option<egui::Pos2> {
    if !safe_clip.is_positive() || size.x > safe_clip.width() || size.y > safe_clip.height() {
        return None;
    }
    let guard = egui::Rect::from_center_size(anchor, BILLBOARD_ANCHOR_GUARD);
    let candidates = [
        egui::pos2(
            anchor.x - size.x * 0.5,
            anchor.y - size.y - BILLBOARD_LABEL_GAP,
        ),
        egui::pos2(anchor.x + BILLBOARD_LABEL_GAP, anchor.y - size.y * 0.5),
        egui::pos2(
            anchor.x - size.x - BILLBOARD_LABEL_GAP,
            anchor.y - size.y * 0.5,
        ),
        egui::pos2(anchor.x - size.x * 0.5, anchor.y + BILLBOARD_LABEL_GAP),
    ];

    candidates.into_iter().find_map(|top_left| {
        let max = safe_clip.max - size;
        let min = safe_clip.min;
        let clamped = egui::pos2(
            top_left.x.clamp(min.x, max.x),
            top_left.y.clamp(min.y, max.y),
        );
        let label = egui::Rect::from_min_size(clamped, size);
        let backdrop = label.expand2(BILLBOARD_BACKDROP_PADDING);
        (!backdrop.intersects(guard) && occupied.iter().all(|other| !backdrop.intersects(*other)))
            .then_some(clamped)
    })
}

#[cfg(test)]
mod tests {
    use super::{place_label, render_anchor};
    use bevy::prelude::*;
    use bevy_egui::egui;

    #[test]
    fn billboard_anchor_is_derived_from_the_render_pose() {
        let gtf = GlobalTransform::from(Transform::from_xyz(4.0, 2.0, -7.0));
        assert_eq!(render_anchor(&gtf, 3.0), Vec3::new(4.0, 5.0, -7.0));
    }

    #[test]
    fn labels_are_clamped_inside_the_viewport() {
        let clip = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(300.0, 200.0));
        let safe_clip = clip.shrink(11.0);
        let top_left = place_label(
            egui::pos2(390.0, 100.0),
            egui::vec2(100.0, 24.0),
            safe_clip,
            &[],
        )
        .expect("a label smaller than the viewport must have a bounded slot");
        let label = egui::Rect::from_min_size(top_left, egui::vec2(100.0, 24.0))
            .expand2(super::BILLBOARD_BACKDROP_PADDING);
        assert!(clip.contains(label.min));
        assert!(clip.contains(label.max));
    }

    #[test]
    fn labels_do_not_share_a_backdrop_or_cover_their_marker() {
        let clip = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 300.0));
        let first = place_label(egui::pos2(250.0, 180.0), egui::vec2(90.0, 24.0), clip, &[])
            .expect("first label must have a slot");
        let first_bg = egui::Rect::from_min_size(first, egui::vec2(90.0, 24.0))
            .expand2(super::BILLBOARD_BACKDROP_PADDING);
        let second = place_label(
            egui::pos2(250.0, 180.0),
            egui::vec2(90.0, 24.0),
            clip,
            &[first_bg],
        )
        .expect("a nearby label must use a different slot");
        let second_bg = egui::Rect::from_min_size(second, egui::vec2(90.0, 24.0))
            .expand2(super::BILLBOARD_BACKDROP_PADDING);
        assert!(!first_bg.intersects(second_bg));
        assert!(!second_bg.intersects(egui::Rect::from_center_size(
            egui::pos2(250.0, 180.0),
            super::BILLBOARD_ANCHOR_GUARD,
        )));
    }
}
