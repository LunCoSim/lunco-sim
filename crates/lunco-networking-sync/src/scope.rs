//! Replication lifetime admission at the scene/transport boundary.
use bevy::prelude::*;
use lunco_core_session::{ReplicatedSceneOwner, ReplicationScope};
pub use lunco_networking_scenario::WireSceneScope;

pub fn wire_scope(scope: ReplicationScope) -> WireSceneScope {
    match scope {
        ReplicationScope::Application => WireSceneScope::Application,
        ReplicationScope::Twin(owner) => WireSceneScope::Twin {
            mount_id: owner.raw(),
        },
    }
}
pub fn internal_scope(scope: WireSceneScope) -> Option<ReplicationScope> {
    match scope {
        WireSceneScope::Application => Some(ReplicationScope::Application),
        WireSceneScope::Twin { mount_id } if mount_id != 0 => Some(ReplicationScope::Twin(
            lunco_workspace::TwinId::new(mount_id),
        )),
        WireSceneScope::Twin { .. } => None,
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub struct SceneScopeFacts<'w, 's> {
    workspace: Option<Res<'w, lunco_workspace::WorkspaceResource>>,
    roots: Option<Res<'w, lunco_assets_core::TwinRoots>>,
    assets: Option<Res<'w, AssetServer>>,
    scenes: Query<
        'w,
        's,
        &'static lunco_usd_bevy_scene::UsdPrimPath,
        With<lunco_usd_bevy_scene::UsdSceneRoot>,
    >,
}
impl SceneScopeFacts<'_, '_> {
    /// Pin an authenticated peer to the hosted document mount, without choosing
    /// a document or guessing an authority from a display name.
    pub fn host_owner(&self, connection: Entity) -> Option<lunco_core_session::ReplicationOwner> {
        match self.host_document_scope()? {
            ReplicationScope::Application => {
                Some(lunco_core_session::ReplicationOwner::Application { connection })
            }
            ReplicationScope::Twin(host_twin) => {
                let twin = self.workspace.as_ref()?.twin(host_twin)?;
                let roots = self.roots.as_ref()?;
                let authority = if self.scenes.is_empty() {
                    let names = match roots.names() {
                        Ok(names) => names,
                        Err(error) => {
                            warn_once!("[net] cannot inspect hosted Twin mounts: {error}");
                            return None;
                        }
                    };
                    let mut matching = None;
                    for authority in names {
                        match roots.root_for(&authority) {
                            Ok(Some(root)) if root == twin.root => {
                                matching = Some(authority);
                                break;
                            }
                            Ok(_) => {}
                            Err(error) => {
                                warn_once!("[net] cannot validate hosted Twin mount: {error}");
                                return None;
                            }
                        }
                    }
                    matching?
                } else {
                    self.authority()?
                };
                let root = match roots.root_for(&authority) {
                    Ok(Some(root)) if root == twin.root => root,
                    _ => {
                        warn_once!("[net] hosted journal has no matching live asset mount");
                        return None;
                    }
                };
                Some(lunco_core_session::ReplicationOwner::Twin {
                    scene: ReplicatedSceneOwner {
                        connection,
                        host_twin,
                        authority,
                        root,
                        owns_mount: false,
                    },
                })
            }
        }
    }

    pub fn client_owner(
        &self,
        connection: Option<Entity>,
        scene: Option<&ReplicatedSceneOwner>,
        announced: Option<lunco_workspace::TwinId>,
    ) -> Option<lunco_core_session::ReplicationOwner> {
        match self.client_scope(connection, scene, announced)? {
            ReplicationScope::Application => {
                Some(lunco_core_session::ReplicationOwner::Application {
                    connection: connection?,
                })
            }
            ReplicationScope::Twin(_) => Some(lunco_core_session::ReplicationOwner::Twin {
                scene: scene?.clone(),
            }),
        }
    }

    fn authority(&self) -> Option<String> {
        let prim = self.scenes.single().ok()?;
        let path = self.assets.as_ref()?.get_path(prim.stage_handle.id())?;
        let uri = lunco_assets_core::asset_path::anchor_of(&path);
        lunco_assets_core::parse_twin_uri(&uri).map(|(authority, _)| authority.to_owned())
    }
    pub(crate) fn application_is_live(&self) -> bool {
        self.workspace
            .as_ref()
            .and_then(|workspace| workspace.active_twin)
            .is_none()
            && self.scenes.is_empty()
    }
    /// Pin outgoing document work at its workspace admission boundary.
    pub fn host_document_scope(&self) -> Option<ReplicationScope> {
        if let Some(owner) = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.active_twin)
        {
            self.workspace.as_ref()?.twin(owner)?;
            Some(ReplicationScope::Twin(owner))
        } else if self.application_is_live() {
            Some(ReplicationScope::Application)
        } else {
            None
        }
    }
    /// A retained journal keeps its declared binding after a scene closes.
    pub fn host_journal_scope(
        &self,
        journal: &lunco_doc_bevy::JournalResource,
        application: &ApplicationJournalBinding,
    ) -> Option<ReplicationScope> {
        let scope = self.host_document_scope()?;
        let identity = journal.with_read(|journal| journal.twin().clone());
        match scope {
            ReplicationScope::Application => {
                (application.0.as_ref() == Some(&identity)).then_some(scope)
            }
            ReplicationScope::Twin(owner) => (identity.0
                == self.workspace.as_ref()?.twin(owner)?.root.to_string_lossy())
            .then_some(scope),
        }
    }
    /// Scene state is admitted only after the actual root matches its mount.
    pub fn host_scene_scope(&self) -> Option<ReplicationScope> {
        let scope = self.host_document_scope()?;
        match scope {
            ReplicationScope::Application => Some(scope),
            ReplicationScope::Twin(owner) => {
                let root = match self.roots.as_ref()?.root_for(&self.authority()?) {
                    Ok(Some(root)) => root,
                    Ok(None) => {
                        error_once!("[net] active USD root references a retired Twin authority");
                        return None;
                    }
                    Err(error) => {
                        error_once!("[net] cannot validate USD root mount: {error}");
                        return None;
                    }
                };
                (root == self.workspace.as_ref()?.twin(owner)?.root).then_some(scope)
            }
        }
    }
    pub fn client_journal_scope(
        &self,
        connection: Option<Entity>,
        owner: Option<&ReplicatedSceneOwner>,
        announced: Option<lunco_workspace::TwinId>,
        journal: &lunco_doc_bevy::JournalResource,
        application: &ApplicationJournalBinding,
    ) -> Option<ReplicationScope> {
        let scope = self.client_scope(connection, owner, announced)?;
        if scope == ReplicationScope::Application {
            self.host_journal_scope(journal, application)
        } else {
            Some(scope)
        }
    }
    pub fn client_scope(
        &self,
        connection: Option<Entity>,
        owner: Option<&ReplicatedSceneOwner>,
        announced: Option<lunco_workspace::TwinId>,
    ) -> Option<ReplicationScope> {
        let connection = connection?;
        if let Some(owner) = owner {
            if owner.connection != connection || announced != Some(owner.host_twin) {
                return None;
            }
            if self.authority().as_deref() != Some(owner.authority.as_str()) {
                return None;
            }
            let root = match self.roots.as_ref()?.root_for(&owner.authority) {
                Ok(Some(root)) => root,
                Ok(None) => {
                    error_once!("[net] remote scene references a retired Twin authority");
                    return None;
                }
                Err(error) => {
                    error_once!("[net] cannot validate remote scene mount: {error}");
                    return None;
                }
            };
            return (root == owner.root).then_some(ReplicationScope::Twin(owner.host_twin));
        }
        (announced.is_none() && self.application_is_live()).then_some(ReplicationScope::Application)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn application_journal_binding_does_not_inherit_in_place_twin_identity() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let identity = lunco_twin_journal::TwinId::new("generic-application");
        let journal = lunco_doc_bevy::JournalResource::new(
            identity.clone(),
            lunco_twin_journal::AuthorId::local(),
        );
        world.insert_resource(journal.clone());
        world.insert_resource(ApplicationJournalBinding(Some(identity)));
        let admitted = world
            .run_system_once(
                |facts: SceneScopeFacts,
                 journal: Res<lunco_doc_bevy::JournalResource>,
                 binding: Res<ApplicationJournalBinding>| {
                    facts.host_journal_scope(&journal, &binding)
                },
            )
            .unwrap();
        assert_eq!(admitted, Some(ReplicationScope::Application));
        journal.with_write(|journal| {
            journal.set_twin(lunco_twin_journal::TwinId::new("generic-twin"))
        });
        let rejected = world
            .run_system_once(
                |facts: SceneScopeFacts,
                 journal: Res<lunco_doc_bevy::JournalResource>,
                 binding: Res<ApplicationJournalBinding>| {
                    facts.host_journal_scope(&journal, &binding)
                },
            )
            .unwrap();
        assert_eq!(rejected, None);
    }
    #[test]
    fn wire_scope_requires_explicit_nonzero_twin_owner() {
        let owner = lunco_workspace::TwinId::new(7);
        assert_eq!(
            internal_scope(wire_scope(ReplicationScope::Twin(owner))),
            Some(ReplicationScope::Twin(owner))
        );
        assert_eq!(internal_scope(WireSceneScope::Twin { mount_id: 0 }), None);
        assert_ne!(
            internal_scope(WireSceneScope::Application),
            Some(ReplicationScope::Twin(owner))
        );
    }
}

/// Durable journal explicitly admitted at the Application resource boundary.
/// Captured once on resource insertion; an in-place Twin bind cannot inherit it.
#[derive(Resource, Default)]
pub struct ApplicationJournalBinding(pub Option<lunco_twin_journal::TwinId>);
pub(crate) fn bind_application_journal(
    facts: SceneScopeFacts,
    journal: Option<Res<lunco_doc_bevy::JournalResource>>,
    mut binding: ResMut<ApplicationJournalBinding>,
) {
    binding.0 = if facts.application_is_live() {
        journal.map(|journal| journal.with_read(|journal| journal.twin().clone()))
    } else {
        None
    };
}
