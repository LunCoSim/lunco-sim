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
//! ## Two things this must get right
//!
//! **big_space.** Positions come from [`lunco_core::coords::world_position`]
//! (`cell × edge + local`), and projection is done camera-RELATIVE: the offset
//! from camera to label is computed in `f64`, narrowed to `f32` only once it is
//! small, and added to the camera's own transform. Projecting an absolute
//! world position instead would lose metres of precision at kilometre range —
//! the exact failure the float-origin hierarchy exists to prevent — and labels
//! would visibly swim against the geometry they name.
//!
//! **Depth.** egui paints over everything, so a label whose subject is behind a
//! ridge would otherwise still be readable. Labels are drawn nearest-last, and
//! each is dropped once its subject passes `fade_end`. True occlusion would
//! need a depth read this overlay does not have; the honest mitigation is the
//! distance cut plus a backdrop chip so text never dissolves into terrain.

use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_usd_sim::billboard::{render_billboard, BillboardFacts, UsdBillboard};

/// Paint every visible [`UsdBillboard`].
#[allow(clippy::too_many_arguments)]
pub fn draw_billboard_overlay(
    q_billboards: Query<(Entity, &UsdBillboard, &Name, Option<&ViewVisibility>)>,
    q_camera: Query<(Entity, &Camera, &GlobalTransform), With<Camera3d>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::grid::cell::CellCoord>, &Transform)>,
    q_site: Query<&lunco_celestial::GeodeticAnchor, With<lunco_celestial::SiteAnchor>>,
    q_bodies: Query<&lunco_celestial::CelestialBody>,
    mut egui_ctx: bevy_egui::EguiContexts,
    theme: Option<Res<lunco_theme::Theme>>,
) {
    if q_billboards.is_empty() {
        return;
    }
    let Some((cam_entity, camera, cam_gtf)) = q_camera.iter().find(|(_, c, _)| c.is_active) else {
        return;
    };
    let Ok(ctx) = egui_ctx.ctx_mut() else { return };
    let origin = ctx.content_rect().min.to_vec2();
    let theme = theme.map(|t| t.clone()).unwrap_or_else(lunco_theme::Theme::dark);

    let cam_world =
        lunco_core::coords::world_position(cam_entity, &q_parents, &q_grids, &q_spatial)
            .unwrap_or(DVec3::ZERO);

    // Site anchor + body radius, resolved ONCE — every label on screen shares
    // them, and they cannot change within a frame.
    let site = q_site.iter().next().copied();
    let radius_m = site.and_then(|a| {
        q_bodies.iter().find(|b| b.ephemeris_id == a.body).map(|b| b.radius_m)
    });

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("usd_billboard_overlay"),
    ));

    // Collect first so we can paint far-to-near: with no depth buffer, drawing
    // nearest LAST is what keeps a close label on top of a distant one.
    struct Drawn {
        screen: egui::Pos2,
        text: String,
        distance: f64,
    }
    let mut drawn: Vec<Drawn> = Vec::new();

    for (entity, bb, name, vis) in &q_billboards {
        // An entity culled or explicitly hidden must not keep a floating label.
        if vis.is_some_and(|v| !v.get()) {
            continue;
        }
        let Some(pos) =
            lunco_core::coords::world_position(entity, &q_parents, &q_grids, &q_spatial)
        else {
            continue;
        };
        let anchor_world = pos + DVec3::Y * bb.offset_y as f64;
        let distance = (anchor_world - cam_world).length();
        if distance > bb.fade_end as f64 {
            continue;
        }

        // Camera-relative projection — see the module header.
        let cam_relative = (anchor_world - cam_world).as_vec3();
        let Ok(viewport) = camera.world_to_viewport(cam_gtf, cam_gtf.translation() + cam_relative)
        else {
            continue; // behind the camera
        };

        // The prim's leaf name — `Name` holds the full USD path.
        let leaf = name.as_str().rsplit('/').next().unwrap_or(name.as_str());
        let geo = match (site, radius_m) {
            (Some(a), Some(r)) => {
                Some(lunco_celestial::geo::local_to_geodetic(&a.geodetic, r, pos))
            }
            _ => None,
        };
        let text = render_billboard(
            &bb.template,
            &BillboardFacts { name: leaf, label: None, geo },
        );
        drawn.push(Drawn { screen: egui::pos2(viewport.x, viewport.y) + origin, text, distance });
    }

    drawn.sort_by(|a, b| b.distance.total_cmp(&a.distance));

    for d in &drawn {
        // Fade with distance so far labels recede instead of all shouting
        // equally; never fully transparent before `fade_end` drops it outright.
        let fade = (1.0 - (d.distance as f32 / 1200.0)).clamp(0.25, 1.0);
        let alpha = (255.0 * fade) as u8;
        let c = theme.tokens.text;
        let color = egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha);

        let galley = painter.layout_no_wrap(
            d.text.clone(),
            egui::FontId::proportional(13.0),
            color,
        );
        let size = galley.size();
        let top_left = d.screen - egui::vec2(size.x * 0.5, size.y + 8.0);
        let bg = egui::Rect::from_min_size(top_left, size).expand2(egui::vec2(5.0, 3.0));
        painter.rect_filled(bg, 3.0, egui::Color32::from_black_alpha((170.0 * fade) as u8));
        painter.galley(top_left, galley, color);
    }
}
