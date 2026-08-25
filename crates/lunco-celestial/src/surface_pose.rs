//! Canonical user-facing coordinates for entities in a site-anchored scene.
//!
//! A BigSpace root pose is a storage/render fact, not a surface coordinate.
//! Celestial ancestors translate and rotate as ephemeris time advances, so a
//! root-frame `DVec3` cannot be interpreted as site ENU or body-fixed state.
//! This module is the single crossing from the BigSpace hierarchy into the two
//! surface frames users and engineering models actually consume:
//!
//! - [`SitePosition`] — metres in the authored site frame (east, up, north);
//! - [`BodyFixedPosition`] — metres in the body's IAU/WGCCRE fixed frame.
//!
//! Both are derived from the same hierarchy sample. Missing or duplicate site
//! anchors and missing or ambiguous semantic body frames fail closed.

use bevy::ecs::query::QueryFilter;
use bevy::ecs::system::SystemParam;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};

use crate::geo::{body_fixed_to_geodetic, Geodetic, GeodeticAnchor, SiteAnchor};
use crate::registry::{CelestialBodyRegistry, ReferenceFrame, ReferenceFrameIndex};

/// Position in the scene's authored topocentric frame.
///
/// Axes are east = `+X`, up = `+Y`, north = `-Z`, in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SitePosition(pub DVec3);

/// Position in a body's rotating IAU/WGCCRE frame, in metres from its centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyFixedPosition(pub DVec3);

/// One entity pose resolved in both surface coordinate systems.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfacePose {
    /// The unique site anchor that defines [`Self::site_position`].
    pub site: Entity,
    /// NAIF ephemeris id of the surface body.
    pub body: i32,
    /// Position relative to the authored site origin and axes.
    pub site_position: SitePosition,
    /// Entity orientation relative to the authored site axes.
    pub site_rotation: DQuat,
    /// Position relative to the body's centre in rotating body-fixed axes.
    pub body_fixed_position: BodyFixedPosition,
    /// Entity orientation in rotating body-fixed axes.
    pub body_fixed_rotation: DQuat,
    /// Standard spherical geodetic coordinates derived from the body-fixed pose.
    pub geodetic: Geodetic,
}

/// Resolve one entity's canonical surface pose from explicit frame ownership.
///
/// This free function is shared with exclusive `&mut World` bridges such as
/// scripting. Ordinary Bevy systems should take [`SurfacePoseQuery`] instead.
#[allow(clippy::too_many_arguments)]
pub fn resolve_surface_pose<F: QueryFilter>(
    entity: Entity,
    sites: &Query<(Entity, &GeodeticAnchor), With<SiteAnchor>>,
    registry: &CelestialBodyRegistry,
    frame_index: &ReferenceFrameIndex,
    parents: &Query<&ChildOf>,
    grids: &Query<&Grid>,
    spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<SurfacePose> {
    // Multiple site anchors are not one coordinate frame. Selecting the first
    // would make telemetry depend on archetype/load order, so fail closed.
    let (site, anchor) = sites.single().ok()?;
    let descriptor = registry.get(anchor.body)?;
    let body_grid = frame_index.resolve(ReferenceFrame::BodyFixed { body: anchor.body })?;

    let (
        _,
        entity_common_position,
        entity_common_rotation,
        site_common_position,
        site_common_rotation,
    ) = lunco_core::coords::common_grid_poses(entity, site, parents, grids, spatial)?;
    let common_to_site = site_common_rotation.inverse();
    let site_position = common_to_site * (entity_common_position - site_common_position);
    let site_rotation = (common_to_site * entity_common_rotation).normalize();

    let (body_fixed_position, body_fixed_rotation) =
        lunco_core::coords::pose_in_grid(entity, body_grid, parents, grids, spatial)?;

    Some(SurfacePose {
        site,
        body: anchor.body,
        site_position: SitePosition(site_position),
        site_rotation,
        body_fixed_position: BodyFixedPosition(body_fixed_position),
        body_fixed_rotation: body_fixed_rotation.normalize(),
        geodetic: body_fixed_to_geodetic(body_fixed_position, descriptor.radius_m),
    })
}

/// Read-only Bevy parameter for canonical site/body-fixed coordinates.
#[derive(SystemParam)]
pub struct SurfacePoseQuery<'w, 's> {
    sites: Query<'w, 's, (Entity, &'static GeodeticAnchor), With<SiteAnchor>>,
    registry: Option<Res<'w, CelestialBodyRegistry>>,
    frame_index: Option<Res<'w, ReferenceFrameIndex>>,
    parents: Query<'w, 's, &'static ChildOf>,
    grids: Query<'w, 's, &'static Grid>,
    spatial: Query<'w, 's, (Option<&'static CellCoord>, &'static Transform)>,
}

impl SurfacePoseQuery<'_, '_> {
    /// Number of authored site frames visible to this query.
    ///
    /// Consumers may use zero to select an explicitly documented non-celestial
    /// coordinate mode. Any value above one is an ambiguity and must not fall
    /// back to root-world coordinates.
    pub fn site_count(&self) -> usize {
        self.sites.iter().count()
    }

    /// Resolve `entity`, or `None` when the scene does not define one complete,
    /// unambiguous site/body-fixed frame.
    pub fn get(&self, entity: Entity) -> Option<SurfacePose> {
        resolve_surface_pose(
            entity,
            &self.sites,
            self.registry.as_deref()?,
            self.frame_index.as_deref()?,
            &self.parents,
            &self.grids,
            &self.spatial,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::{geodetic_to_body_fixed, LocalTangentFrame};
    use crate::registry::{update_reference_frame_index, MOON_MEAN_RADIUS_M};

    fn scene_to_body_rotation(anchor: &Geodetic) -> DQuat {
        let tangent = LocalTangentFrame::body_fixed(anchor, MOON_MEAN_RADIUS_M);
        DQuat::from_mat3(&bevy::math::DMat3::from_cols(
            tangent.east,
            tangent.up,
            -tangent.north,
        ))
    }

    fn read_pose(app: &mut App, entity: Entity) -> Option<SurfacePose> {
        let system = app
            .world_mut()
            .register_system(move |query: SurfacePoseQuery| query.get(entity));
        app.world_mut().run_system(system).ok().flatten()
    }

    #[test]
    fn surface_pose_is_invariant_to_every_ancestor_pose_and_cell() {
        let mut app = App::new();
        app.init_resource::<ReferenceFrameIndex>()
            .insert_resource(CelestialBodyRegistry::default_system())
            .add_systems(First, update_reference_frame_index);

        let root = app
            .world_mut()
            .spawn((
                lunco_core::WorldGridConfig::default().grid(),
                GlobalTransform::default(),
            ))
            .id();
        let inertial = app
            .world_mut()
            .spawn((
                lunco_core::WorldGridConfig::default().grid(),
                CellCoord::new(75_000_000, -4_000, 11_000),
                Transform::from_rotation(Quat::from_rotation_z(0.4)),
                ChildOf(root),
            ))
            .id();
        let body_grid = app
            .world_mut()
            .spawn((
                ReferenceFrame::BodyFixed {
                    body: crate::ephemeris_id::MOON,
                },
                lunco_core::WorldGridConfig::default().grid(),
                CellCoord::new(192_000, 2_000, -8_000),
                Transform::from_rotation(Quat::from_rotation_y(0.7)),
                ChildOf(inertial),
            ))
            .id();
        let surface_grid = app
            .world_mut()
            .spawn((
                lunco_core::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::IDENTITY,
                ChildOf(body_grid),
            ))
            .id();

        let anchor = Geodetic::new(25.2853, -52.3712, -2169.0);
        let anchor_position = geodetic_to_body_fixed(&anchor, MOON_MEAN_RADIUS_M);
        let anchor_rotation = scene_to_body_rotation(&anchor);
        let surface = app.world().get::<Grid>(surface_grid).unwrap();
        let (site_cell, site_local) = surface.translation_to_grid(anchor_position);
        let site = app
            .world_mut()
            .spawn((
                SiteAnchor,
                GeodeticAnchor {
                    body: crate::ephemeris_id::MOON,
                    geodetic: anchor,
                },
                site_cell,
                Transform::from_translation(site_local).with_rotation(anchor_rotation.as_quat()),
                ChildOf(surface_grid),
            ))
            .id();
        let rover_local = DVec3::new(123.25, 0.7, -88.5);
        let rover = app
            .world_mut()
            .spawn((
                Transform::from_translation(rover_local.as_vec3())
                    .with_rotation(Quat::from_rotation_y(0.2)),
                ChildOf(site),
            ))
            .id();

        app.update();
        let before = read_pose(&mut app, rover).expect("surface pose must resolve");
        let authored_roundtrip_error = (before.site_position.0 - rover_local).length();
        assert!(
            authored_roundtrip_error < 1.0e-3,
            "cell/local f32 terminal remainder exceeded 1 mm: {authored_roundtrip_error} m"
        );

        // Change every ancestor representation above the body frame. Neither
        // site-local nor body-fixed/geodetic state may observe it.
        {
            let mut entity = app.world_mut().entity_mut(inertial);
            *entity.get_mut::<CellCoord>().unwrap() = CellCoord::new(-91_000_000, 13_000, 42_000);
            let mut transform = entity.get_mut::<Transform>().unwrap();
            transform.translation = Vec3::new(742.0, -311.0, 93.0);
            transform.rotation = Quat::from_rotation_x(-1.1);
        }
        {
            let mut body_entity = app.world_mut().entity_mut(body_grid);
            let mut transform = body_entity.get_mut::<Transform>().unwrap();
            transform.rotation = Quat::from_rotation_y(-2.2);
        }
        app.update();

        let after = read_pose(&mut app, rover).expect("surface pose must remain resolvable");
        assert!((after.site_position.0 - before.site_position.0).length() < 1.0e-9);
        assert!(
            after
                .site_rotation
                .angle_between(before.site_rotation)
                .abs()
                < 1.0e-12
        );
        assert!((after.body_fixed_position.0 - before.body_fixed_position.0).length() < 1.0e-9);
        assert!(
            after
                .body_fixed_rotation
                .angle_between(before.body_fixed_rotation)
                .abs()
                < 1.0e-12
        );
        assert_eq!(after.geodetic, before.geodetic);
    }

    #[test]
    fn duplicate_sites_are_not_resolved_by_iteration_order() {
        let mut app = App::new();
        app.init_resource::<ReferenceFrameIndex>()
            .insert_resource(CelestialBodyRegistry::default_system())
            .add_systems(First, update_reference_frame_index);
        let root = app
            .world_mut()
            .spawn((
                lunco_core::WorldGridConfig::default().grid(),
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut().spawn((
            ReferenceFrame::BodyFixed {
                body: crate::ephemeris_id::MOON,
            },
            lunco_core::WorldGridConfig::default().grid(),
            CellCoord::ZERO,
            Transform::IDENTITY,
            ChildOf(root),
        ));
        for longitude in [0.0, 1.0] {
            app.world_mut().spawn((
                SiteAnchor,
                GeodeticAnchor {
                    body: crate::ephemeris_id::MOON,
                    geodetic: Geodetic::new(0.0, longitude, 0.0),
                },
                Transform::IDENTITY,
                ChildOf(root),
            ));
        }
        let rover = app
            .world_mut()
            .spawn((Transform::IDENTITY, ChildOf(root)))
            .id();
        app.update();
        assert_eq!(read_pose(&mut app, rover), None);
    }
}
