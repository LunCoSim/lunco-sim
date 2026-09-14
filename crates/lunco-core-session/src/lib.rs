//! Always-on session and authority substrate for LunCoSim.
//!
//! This crate is the runtime owner of session state, possession/RBAC policy,
//! prediction markers, and the identity-admission systems that depend on the
//! current network role. It depends on the lower-level [`lunco_core`] types;
//! the core crate remains independent of this session layer.

extern crate self as lunco_core_session;

use bevy::prelude::*;

pub mod session;

pub use session::*;

/// Installs the always-on session substrate and the systems that connect it to
/// core identity and scene-lifecycle mechanisms.
pub struct LunCoCoreSessionPlugin;

impl Plugin for LunCoCoreSessionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkRole>()
            .init_resource::<LocalSession>()
            .init_resource::<SyncApplyGuard>()
            .init_resource::<NetStatus>()
            .init_resource::<SessionRegistry>()
            .init_resource::<SessionProfiles>()
            .init_resource::<SessionRbac>()
            .init_resource::<CommandPolicyRegistry>()
            .init_resource::<PendingReplicatedSpawns>()
            .init_resource::<OwnedInputLog>()
            .init_resource::<BufferedClientInputs>()
            .init_resource::<LocalDriveInput>()
            .init_resource::<AppliedInputSeq>()
            .add_systems(lunco_core::SceneTeardown, reset_session_scene_state)
            .add_systems(PostUpdate, assign_global_entity_ids)
            .add_systems(FixedFirst, sync_applied_seq_owners);
    }
}

/// Clear session-owned simulation state before the outgoing scene is removed.
fn reset_session_scene_state(
    mut owned: ResMut<OwnedInputLog>,
    mut buffered: ResMut<BufferedClientInputs>,
    mut local_drive: ResMut<LocalDriveInput>,
    mut applied: ResMut<AppliedInputSeq>,
) {
    owned.0.clear();
    buffered.pending.clear();
    buffered.applied.clear();
    buffered.last_writes.clear();
    local_drive.0.clear();
    applied.retain_gids(|_| false);
}

/// Keep input acknowledgements keyed to the current authoritative owner.
pub fn sync_applied_seq_owners(
    role: Res<NetworkRole>,
    registry: Res<SessionRegistry>,
    mut applied: ResMut<AppliedInputSeq>,
    mut buffered: ResMut<BufferedClientInputs>,
) {
    if !role.is_host() || !registry.is_changed() {
        return;
    }
    for gid in applied.changed_owner_gids(&registry) {
        buffered.clear_gid(gid);
    }
    applied.sync_owners(&registry);
}

/// Admit identity-bearing runtime entities after their provenance and role
/// markers have been authored.
fn assign_global_entity_ids(
    mut commands: Commands,
    q_new: Query<
        (
            Entity,
            Option<&lunco_core::Provenance>,
            Has<SkipContentStamp>,
        ),
        (
            Without<lunco_core::GlobalEntityId>,
            Or<(With<lunco_core::Provenance>, With<SkipContentStamp>)>,
        ),
    >,
    role: Res<NetworkRole>,
) {
    let is_authoritative = role.is_authoritative();
    for (entity, prov, runtime_instance) in q_new.iter() {
        if runtime_instance {
            if is_authoritative {
                commands
                    .entity(entity)
                    .try_insert(lunco_core::GlobalEntityId::allocate_authoritative());
            }
            continue;
        }
        let Some(prov) = prov else {
            continue;
        };
        match prov {
            lunco_core::Provenance::Local => {}
            p @ (lunco_core::Provenance::Content { .. }
            | lunco_core::Provenance::Derived { .. }) => {
                if let Some(id) = lunco_core::identity::derive_id(p) {
                    commands
                        .entity(entity)
                        .try_insert(lunco_core::GlobalEntityId::from_raw(id));
                }
            }
            lunco_core::Provenance::Authoritative => {
                if is_authoritative {
                    commands
                        .entity(entity)
                        .try_insert(lunco_core::GlobalEntityId::allocate_authoritative());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_initializes_session_substrate() {
        let mut app = App::new();
        app.add_plugins(LunCoCoreSessionPlugin);
        let world = app.world();
        assert!(world.get_resource::<NetworkRole>().is_some());
        assert!(world.get_resource::<LocalSession>().is_some());
        assert!(world.get_resource::<SyncApplyGuard>().is_some());
        assert!(world.get_resource::<NetStatus>().is_some());
        assert!(world.get_resource::<SessionRegistry>().is_some());
        assert!(world.get_resource::<PendingReplicatedSpawns>().is_some());
        assert!(world.get_resource::<OwnedInputLog>().is_some());
        assert!(world.get_resource::<AppliedInputSeq>().is_some());
    }

    #[test]
    fn identity_admission_uses_provenance_and_authority_role() {
        let mut app = App::new();
        app.add_plugins(LunCoCoreSessionPlugin);
        let content = lunco_core::identity::content("usd", "scene.usda", "/World/Rover");
        let expected = lunco_core::identity::derive_id(&content).unwrap();
        let content_entity = app.world_mut().spawn(content).id();
        let local_entity = app.world_mut().spawn(lunco_core::Provenance::Local).id();
        app.world_mut().run_schedule(PostUpdate);

        assert_eq!(
            app.world()
                .get::<lunco_core::GlobalEntityId>(content_entity)
                .map(lunco_core::GlobalEntityId::get),
            Some(expected)
        );
        assert!(app
            .world()
            .get::<lunco_core::GlobalEntityId>(local_entity)
            .is_none());
    }

    #[test]
    fn session_scene_teardown_clears_input_state() {
        let mut app = App::new();
        app.add_plugins(LunCoCoreSessionPlugin);
        app.world_mut()
            .resource_mut::<OwnedInputLog>()
            .0
            .insert(1, VesselInputLog::default());
        app.world_mut().resource_mut::<BufferedClientInputs>().push(
            1,
            1,
            vec![("throttle".into(), 1.0)],
        );
        app.world_mut()
            .resource_mut::<LocalDriveInput>()
            .0
            .insert(1, (1.0, 0.0));
        app.world_mut()
            .resource_mut::<AppliedInputSeq>()
            .record(1, None, 1);

        app.world_mut().run_schedule(lunco_core::SceneTeardown);

        assert!(app.world().resource::<OwnedInputLog>().0.is_empty());
        let buffered = app.world().resource::<BufferedClientInputs>();
        assert!(buffered.pending.is_empty());
        assert!(buffered.applied.is_empty());
        assert!(buffered.last_writes.is_empty());
        assert!(app.world().resource::<LocalDriveInput>().0.is_empty());
        assert!(app.world().resource::<AppliedInputSeq>().is_empty());
    }
}
