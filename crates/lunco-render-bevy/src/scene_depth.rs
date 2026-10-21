//! Native opaque-depth lifetime for source-declared shader consumers.
//!
//! Source/material/camera topology invalidates this owner. A live mesh shader's
//! capability enables Bevy's existing prepass on 3D views, including previews.
//! Only automatically supplied components are retired with the last consumer.

use bevy::core_pipeline::prepass::DepthPrepass;
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use lunco_materials::ShaderLookSourceInterface;

use crate::ShaderMaterial;

#[derive(Resource, Default)]
struct SceneDepthInvalidation;

#[derive(Component)]
struct AutomaticSceneDepth;

fn added<T: Component>(_event: On<Add<T>>, mut invalidation: ResMut<SceneDepthInvalidation>) {
    invalidation.set_changed();
}

fn inserted<T: Component>(_event: On<Insert<T>>, mut invalidation: ResMut<SceneDepthInvalidation>) {
    invalidation.set_changed();
}

fn removed<T: Component>(_event: On<Remove<T>>, mut invalidation: ResMut<SceneDepthInvalidation>) {
    invalidation.set_changed();
}

pub(crate) fn build(app: &mut App) {
    app.init_resource::<SceneDepthInvalidation>()
        .add_observer(inserted::<ShaderLookSourceInterface>)
        .add_observer(removed::<ShaderLookSourceInterface>)
        // Rebinding the same material's live parameters is not topology.
        .add_observer(added::<MeshMaterial3d<ShaderMaterial>>)
        .add_observer(removed::<MeshMaterial3d<ShaderMaterial>>)
        .add_observer(added::<Mesh3d>)
        .add_observer(removed::<Mesh3d>)
        .add_observer(added::<Camera3d>)
        .add_observer(removed::<Camera3d>)
        .add_observer(added::<DepthPrepass>)
        .add_observer(removed::<DepthPrepass>)
        .add_systems(
            Update,
            reconcile.run_if(resource_changed::<SceneDepthInvalidation>),
        );
}

fn reconcile(
    consumers: Query<
        &ShaderLookSourceInterface,
        (With<Mesh3d>, With<MeshMaterial3d<ShaderMaterial>>),
    >,
    cameras: Query<(Entity, Has<DepthPrepass>, Has<AutomaticSceneDepth>), With<Camera3d>>,
    retired_cameras: Query<Entity, (With<AutomaticSceneDepth>, Without<Camera3d>)>,
    mut commands: Commands,
) {
    let required = consumers.iter().any(|source| {
        source.source_valid
            && source
                .capabilities
                .contains(lunco_materials::dyn_params::VOLUME_DEPTH_CAPABILITY)
    });
    for (camera, has_depth, automatic) in &cameras {
        if required && !has_depth {
            commands
                .entity(camera)
                .try_insert((DepthPrepass, AutomaticSceneDepth));
        } else if !required && automatic {
            commands.entity(camera).try_remove::<DepthPrepass>();
            commands.entity(camera).try_remove::<AutomaticSceneDepth>();
        }
    }
    for camera in &retired_cameras {
        commands
            .entity(camera)
            .try_remove::<(DepthPrepass, AutomaticSceneDepth)>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_lifetime_follows_render_consumers() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        build(&mut app);
        let automatic = app.world_mut().spawn(Camera3d::default()).id();
        let explicit = app
            .world_mut()
            .spawn((Camera3d::default(), DepthPrepass))
            .id();
        let source = ShaderLookSourceInterface {
            source_valid: true,
            capabilities: [lunco_materials::dyn_params::VOLUME_DEPTH_CAPABILITY.to_string()].into(),
            ..default()
        };
        let consumer = app
            .world_mut()
            .spawn((
                Mesh3d::default(),
                MeshMaterial3d::<ShaderMaterial>(Handle::default()),
                source.clone(),
            ))
            .id();
        app.update();
        assert!(app.world().get::<DepthPrepass>(automatic).is_some());
        assert!(app.world().get::<AutomaticSceneDepth>(explicit).is_none());

        let late = app.world_mut().spawn(Camera3d::default()).id();
        app.update();
        assert!(app.world().get::<DepthPrepass>(late).is_some());

        app.world_mut().entity_mut(late).remove::<Camera3d>();
        app.update();
        assert!(app.world().get::<DepthPrepass>(late).is_none());
        assert!(app.world().get::<AutomaticSceneDepth>(late).is_none());

        app.world_mut().entity_mut(consumer).remove::<Mesh3d>();
        app.update();
        assert!(app.world().get::<DepthPrepass>(automatic).is_none());
        app.world_mut()
            .entity_mut(consumer)
            .insert(Mesh3d::default());
        app.update();
        assert!(app.world().get::<DepthPrepass>(automatic).is_some());

        // Removing the draw leaves its independent source facts alive. They
        // must not keep camera resources after the last real consumer retires.
        app.world_mut()
            .entity_mut(consumer)
            .remove::<MeshMaterial3d<ShaderMaterial>>();
        app.update();
        assert!(app.world().get::<DepthPrepass>(automatic).is_none());
        assert!(app.world().get::<DepthPrepass>(late).is_none());
        assert!(app.world().get::<DepthPrepass>(explicit).is_some());

        app.world_mut()
            .entity_mut(consumer)
            .insert(MeshMaterial3d::<ShaderMaterial>(Handle::default()));
        app.update();
        assert!(app.world().get::<DepthPrepass>(automatic).is_some());
        app.world_mut()
            .entity_mut(consumer)
            .insert(ShaderLookSourceInterface {
                source_valid: false,
                ..source.clone()
            });
        app.update();
        assert!(app.world().get::<DepthPrepass>(automatic).is_none());
        app.world_mut().entity_mut(consumer).insert(source);
        app.update();
        assert!(app.world().get::<DepthPrepass>(automatic).is_some());
        app.world_mut().despawn(consumer);
        app.update();
        assert!(app.world().get::<DepthPrepass>(automatic).is_none());
        assert!(app.world().get::<DepthPrepass>(explicit).is_some());
    }
}
