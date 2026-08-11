//! Draws the labels prims asked for via `lunco:billboard*`
//! ([`UsdBillboard`](lunco_usd_sim::billboard::UsdBillboard)).
//!
//! Screen space is the right space for this. A world-space text mesh would have
//! to be re-oriented every frame, would scale itself into illegibility, and
//! would z-fight the terrain it labels; an egui overlay is always camera-facing
//! and always crisp. The same reasoning already produced the checkpoint-number
//! overlay this is modelled on — the difference is that what to write here comes
//! from the scene rather than from Rust.
//!
//! ## Three things this must get right
//!
//! **big_space.** Projection uses the subject's and camera's
//! [`GlobalTransform`] values from the same render frame. Those are the exact
//! camera-relative poses used by the mesh renderer, so a label cannot combine
//! an interpolated render pose with an independently sampled grid pose. The
//! absolute [`lunco_core::coords::world_position`] path is retained only for
//! geodetic text (`{lat}`, `{lon}`, `{height}`), where authored coordinates are
//! intentionally reported in the simulation frame rather than the render frame.
//!
//! **Depth.** egui paints over everything, so a label whose subject is behind a
//! ridge would otherwise still be readable. Labels are drawn nearest-last, and
//! each is dropped once its subject passes `fade_end`. True occlusion would
//! need a depth read this overlay does not have; the honest mitigation is the
//! distance cut plus a backdrop chip so text never dissolves into terrain.
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
use lunco_render::SceneCamera;
use lunco_usd_sim::billboard::{render_billboard, BillboardFacts, UsdBillboard};
use lunco_workbench::{PanelRects, VIEWPORT_PANEL_ID};

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
        Option<&ViewVisibility>,
        Option<&lunco_core::markers::Callsign>,
        &GlobalTransform,
    )>,
    q_camera: Query<(&Camera, &GlobalTransform), (With<Camera3d>, With<SceneCamera>)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::grid::cell::CellCoord>, &Transform)>,
    q_site: Query<&lunco_celestial::GeodeticAnchor, With<lunco_celestial::SiteAnchor>>,
    registry: Option<Res<lunco_celestial::registry::CelestialBodyRegistry>>,
    scene_viewport: Option<Res<lunco_core::SceneViewport>>,
    panel_rects: Option<Res<PanelRects>>,
    mut egui_ctx: bevy_egui::EguiContexts,
    theme: Option<Res<lunco_theme::Theme>>,
) {
    if q_billboards.is_empty() {
        return;
    }
    // A hidden workbench scene must not leave one frame of screen-space labels
    // behind while the camera reconciler turns its window camera off.
    if scene_viewport.is_some_and(|viewport| !viewport.visible) {
        return;
    }
    let Some((camera, cam_gtf)) = q_camera.iter().find(|(c, _)| c.is_active) else {
        return;
    };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let origin = ctx.content_rect().min.to_vec2();
    let clip_rect = panel_rects
        .as_ref()
        .and_then(|rects| rects.egui_rect(VIEWPORT_PANEL_ID, ctx))
        .unwrap_or_else(|| ctx.content_rect());
    let theme = theme
        .map(|t| t.clone())
        .unwrap_or_else(lunco_theme::Theme::dark);

    // Site anchor + body radius, resolved ONCE — every label on screen shares
    // them, and they cannot change within a frame.
    //
    // The radius comes from the body REGISTRY, not from a spawned `CelestialBody`
    // entity. Celestial content is opt-in per scene: a surface scene that anchors
    // to the Moon and never asks for a globe (the Summer Space School twin) spawns
    // no body entity at all, so the entity lookup found nothing and every
    // `{lat}`/`{lon}`/`{height}` on screen rendered `—`. The registry is the same
    // source `sync_terrain_body_curvature` reads to curve the DEM, so the label
    // and the ground now agree on which sphere they are on.
    let site = q_site.iter().next().copied();
    let radius_m = site.zip(registry.as_ref()).and_then(|(a, reg)| {
        reg.bodies
            .iter()
            .find(|b| b.ephemeris_id == a.body)
            .map(|b| b.radius_m)
    });

    // Use the root background paint list, not a second custom Background layer.
    // egui does not guarantee an order between ad-hoc layers that are absent
    // from its Area order map; the system itself is scheduled before the
    // workbench, so appending here gives a deterministic 3D → tag → UI stack.
    let painter = ctx
        .layer_painter(egui::LayerId::background())
        .with_clip_rect(clip_rect);

    // Collect first so we can paint far-to-near: with no depth buffer, drawing
    // nearest LAST is what keeps a close label on top of a distant one.
    struct Drawn {
        screen: egui::Pos2,
        text: String,
        distance: f64,
    }
    let mut drawn: Vec<Drawn> = Vec::new();

    for (entity, bb, name, vis, callsign, gtf) in &q_billboards {
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
        let Some(pos) =
            lunco_core::coords::world_position(entity, &q_parents, &q_grids, &q_spatial)
        else {
            continue;
        };
        let distance = (anchor_render - cam_gtf.translation()).length() as f64;
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

        // The prim's leaf name — `Name` holds the full USD path.
        let leaf = name.as_str().rsplit('/').next().unwrap_or(name.as_str());
        let geo = match (site, radius_m) {
            (Some(a), Some(r)) => Some(lunco_celestial::geo::local_to_geodetic(
                &a.geodetic,
                r,
                pos.0,
            )),
            _ => None,
        };
        let text = render_billboard(
            &bb.template,
            &BillboardFacts {
                name: leaf,
                label: callsign.map(|c| c.0.as_str()),
                geo,
            },
        );
        drawn.push(Drawn {
            screen: egui::pos2(viewport.x, viewport.y) + origin,
            text,
            distance,
        });
    }

    drawn.sort_by(|a, b| b.distance.total_cmp(&a.distance));

    for d in &drawn {
        // Fade with distance so far labels recede instead of all shouting
        // equally; never fully transparent before `fade_end` drops it outright.
        let fade = (1.0 - (d.distance as f32 / 1200.0)).clamp(0.25, 1.0);
        let alpha = (255.0 * fade) as u8;
        let c = theme.tokens.text;
        let color = egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha);

        let galley =
            painter.layout_no_wrap(d.text.clone(), egui::FontId::proportional(13.0), color);
        let size = galley.size();
        let top_left = d.screen - egui::vec2(size.x * 0.5, size.y + 8.0);
        let bg = egui::Rect::from_min_size(top_left, size).expand2(egui::vec2(5.0, 3.0));
        let backdrop = theme.tokens.overlay_backdrop;
        painter.rect_filled(
            bg,
            3.0,
            egui::Color32::from_rgba_unmultiplied(
                backdrop.r(),
                backdrop.g(),
                backdrop.b(),
                (f32::from(backdrop.a()) * fade) as u8,
            ),
        );
        painter.galley(top_left, galley, color);
    }
}

#[cfg(test)]
mod tests {
    use super::render_anchor;
    use bevy::prelude::*;

    #[test]
    fn billboard_anchor_is_derived_from_the_render_pose() {
        let gtf = GlobalTransform::from(Transform::from_xyz(4.0, 2.0, -7.0));
        assert_eq!(render_anchor(&gtf, 3.0), Vec3::new(4.0, 5.0, -7.0));
    }
}
