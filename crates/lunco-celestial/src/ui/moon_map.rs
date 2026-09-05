//! Active lunar-location map presentation.
//!
//! The map is deliberately a small presentation of the canonical celestial
//! pose, not a second coordinate provider. [`SurfacePoseQuery`] resolves the
//! active avatar in the authored site/body-fixed hierarchy; this module only
//! projects the resulting Moon geodetic latitude/longitude into an
//! equirectangular canvas.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{
    Panel, PanelCtx, PanelId, PanelMenuGroup, PanelScrollPolicy, PanelSlot, WorkbenchAppExt,
};

use crate::{ephemeris_id, Geodetic, SurfacePoseQuery};

/// Stable workbench identity for the active lunar-location panel.
pub const MOON_MAP_PANEL_ID: PanelId = PanelId("moon_map");

/// The live location rendered by [`MoonMapPanel`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MoonMapLocation {
    /// Canonical Moon body-fixed geodetic position.
    pub geodetic: Geodetic,
}

/// Change-gated view-model for the active lunar-location panel.
#[derive(Clone, Debug, Default, PartialEq, Resource)]
pub struct MoonMapView {
    /// `None` means that the active avatar has no complete Moon body-fixed
    /// pose. The UI reports that state instead of inventing a map location.
    pub location: Option<MoonMapLocation>,
}

/// Project canonical geodetic coordinates into the panel's equirectangular
/// map. The result is normalized to `[0, 1]` in left-to-right/top-to-bottom
/// order, with longitude wrapping at the `[-180°, 180°)` seam.
pub fn geodetic_to_map_uv(geodetic: Geodetic) -> egui::Pos2 {
    let longitude = normalize_longitude(geodetic.lon_deg);
    let latitude = geodetic.lat_deg.clamp(-90.0, 90.0);
    egui::pos2(
        ((longitude + 180.0) / 360.0) as f32,
        ((90.0 - latitude) / 180.0) as f32,
    )
}

/// Normalize an IAU-east longitude to the map's canonical seam interval.
pub fn normalize_longitude(longitude: f64) -> f64 {
    (longitude + 180.0).rem_euclid(360.0) - 180.0
}

/// Produce the map view from the one authoritative active-avatar pose.
pub fn populate_moon_map_view(
    mut view: ResMut<MoonMapView>,
    avatars: Query<Entity, With<lunco_core::LocalAvatar>>,
    poses: SurfacePoseQuery,
    layout: Option<Res<lunco_workbench::WorkbenchLayout>>,
) {
    // A closed panel has no presentation consumer. Keep the producer cheap,
    // while still allowing standalone/headless view-model tests with no
    // WorkbenchLayout resource to exercise the projection.
    if layout.is_some_and(|layout| !layout.is_panel_docked(MOON_MAP_PANEL_ID)) {
        return;
    }

    let next = avatars
        .single()
        .ok()
        .and_then(|avatar| poses.get(avatar))
        .filter(|pose| pose.body == ephemeris_id::MOON)
        .map(|pose| MoonMapLocation {
            geodetic: pose.geodetic,
        });

    if view.location != next {
        view.location = next;
    }
}

/// Clear the transient marker before a replacement scene integrates.
fn clear_moon_map_on_scene_teardown(mut view: ResMut<MoonMapView>) {
    *view = MoonMapView::default();
}

/// Clear the transient marker when its Twin identity is no longer active.
fn clear_moon_map_on_twin_closed(
    _trigger: On<lunco_workspace::TwinClosed>,
    mut view: ResMut<MoonMapView>,
) {
    *view = MoonMapView::default();
}

/// Dockable map of the active avatar's Moon body-fixed location.
pub struct MoonMapPanel;

impl Panel for MoonMapPanel {
    fn id(&self) -> PanelId {
        MOON_MAP_PANEL_ID
    }

    fn title(&self) -> String {
        "Lunar Map".into()
    }

    fn menu_group(&self) -> PanelMenuGroup {
        PanelMenuGroup::Scene
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }

    fn scroll_policy(&self) -> PanelScrollPolicy {
        PanelScrollPolicy::SelfManaged
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let view = ctx.resource_expect::<MoonMapView>().clone();
        let theme = ctx.resource_expect::<lunco_theme::Theme>().clone();

        ctx.panel_content_frame().show(ui, |ui| {
            ui.heading("Active lunar location");
            ui.label("Moon · IAU/WGCCRE body-fixed · equirectangular");
            ui.add_space(theme.spacing.item_spacing);

            let width = ui.available_width().max(160.0);
            let height = (width * 0.5).clamp(120.0, 260.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
            paint_map(ui, rect, view.location, &theme);

            ui.add_space(theme.spacing.item_spacing);
            if let Some(location) = view.location {
                let geo = location.geodetic;
                ui.label(format!(
                    "Lat {:+.3}° · Lon {:+.3}° · Height {:+.0} m",
                    geo.lat_deg,
                    normalize_longitude(geo.lon_deg),
                    geo.height_m
                ));
            } else {
                ui.label(egui::RichText::new("No active lunar surface location").weak());
                ui.label(
                    egui::RichText::new("The active avatar needs a complete Moon body-fixed pose.")
                        .weak(),
                );
            }
        });
    }
}

fn paint_map(
    ui: &egui::Ui,
    rect: egui::Rect,
    location: Option<MoonMapLocation>,
    theme: &lunco_theme::Theme,
) {
    let painter = ui.painter_at(rect);
    let tokens = &theme.tokens;
    let border = tokens.surface_raised_border;
    let grid = egui::Color32::from_rgba_unmultiplied(border.r(), border.g(), border.b(), 100);
    let axis = egui::Color32::from_rgba_unmultiplied(
        tokens.accent.r(),
        tokens.accent.g(),
        tokens.accent.b(),
        150,
    );

    painter.rect_filled(rect, theme.rounding.window, tokens.surface_sunken);
    painter.rect_stroke(
        rect,
        theme.rounding.window,
        egui::Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );

    for longitude in (-150..=150).step_by(30) {
        let x = map_position(rect, 0.0, longitude as f64).x;
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.0, grid),
        );
    }
    for latitude in (-60..=60).step_by(30) {
        let y = map_position(rect, latitude as f64, 0.0).y;
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            egui::Stroke::new(1.0, grid),
        );
    }

    let equator_y = map_position(rect, 0.0, 0.0).y;
    let meridian_x = map_position(rect, 0.0, 0.0).x;
    painter.line_segment(
        [
            egui::pos2(rect.left(), equator_y),
            egui::pos2(rect.right(), equator_y),
        ],
        egui::Stroke::new(1.5, axis),
    );
    painter.line_segment(
        [
            egui::pos2(meridian_x, rect.top()),
            egui::pos2(meridian_x, rect.bottom()),
        ],
        egui::Stroke::new(1.5, axis),
    );

    if let Some(location) = location {
        let uv = geodetic_to_map_uv(location.geodetic);
        let marker = egui::pos2(
            rect.left() + uv.x * rect.width(),
            rect.top() + uv.y * rect.height(),
        );
        let marker_stroke = egui::Stroke::new(1.5, tokens.text);
        painter.line_segment(
            [
                egui::pos2(marker.x - 9.0, marker.y),
                egui::pos2(marker.x + 9.0, marker.y),
            ],
            marker_stroke,
        );
        painter.line_segment(
            [
                egui::pos2(marker.x, marker.y - 9.0),
                egui::pos2(marker.x, marker.y + 9.0),
            ],
            marker_stroke,
        );
        painter.circle_filled(marker, 5.0, tokens.success);
        painter.circle_stroke(marker, 6.0, egui::Stroke::new(1.5, tokens.text));
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "No lunar position",
            egui::FontId::proportional(13.0),
            tokens.text_subdued,
        );
    }
}

fn map_position(rect: egui::Rect, latitude: f64, longitude: f64) -> egui::Pos2 {
    let uv = geodetic_to_map_uv(Geodetic::new(latitude, longitude, 0.0));
    egui::pos2(
        rect.left() + uv.x * rect.width(),
        rect.top() + uv.y * rect.height(),
    )
}

/// UI plugin for the active lunar-location view.
pub struct MoonMapUiPlugin;

impl Plugin for MoonMapUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MoonMapView>()
            .add_systems(Update, populate_moon_map_view)
            .add_systems(lunco_core::SceneTeardown, clear_moon_map_on_scene_teardown)
            .add_observer(clear_moon_map_on_twin_closed)
            .register_panel(MoonMapPanel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longitude_wraps_at_the_map_seam() {
        assert_eq!(normalize_longitude(180.0), -180.0);
        assert_eq!(normalize_longitude(540.0), -180.0);
        assert!((normalize_longitude(-181.0) - 179.0).abs() < f64::EPSILON);
        assert!((normalize_longitude(181.0) + 179.0).abs() < f64::EPSILON);
    }

    #[test]
    fn equirectangular_projection_has_expected_edges() {
        let north_west = geodetic_to_map_uv(Geodetic::new(90.0, -180.0, 0.0));
        let south_east = geodetic_to_map_uv(Geodetic::new(-90.0, 180.0, 0.0));
        assert_eq!(north_west, egui::pos2(0.0, 0.0));
        assert_eq!(south_east, egui::pos2(0.0, 1.0));
    }

    #[test]
    fn latitude_is_clamped_before_projection() {
        assert_eq!(geodetic_to_map_uv(Geodetic::new(120.0, 0.0, 0.0)).y, 0.0);
        assert_eq!(geodetic_to_map_uv(Geodetic::new(-120.0, 0.0, 0.0)).y, 1.0);
    }
}
