//! Runtime attachment of generic USD programs to projected Bevy owners.
//!
//! Program resolution is a USD runtime concern rather than a visual
//! projection concern. Keeping the live edit and structural refresh bridge
//! here lets the visual crate project scene data without owning executable
//! policy.

use bevy::asset::{AssetId, Assets};
use bevy::prelude::{Entity, World, warn};
use openusd::sdf::Path as SdfPath;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use lunco_usd_bevy_core::program;
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_bevy_stage::{
    UsdInstanceProjection, UsdRead, UsdReadObject, UsdStageAsset, canonical::CanonicalStages,
};

/// Re-read the generic program children of one existing owner.
///
/// This is the structural counterpart to source hot-reload: adding or
/// removing a program prim changes the owner's executable policy, but must not
/// recreate the owner's physics or visual subtree.
pub(crate) fn refresh_program_owner(
    world: &mut World,
    stage_id: AssetId<UsdStageAsset>,
    owner: Entity,
) {
    let Some(network_members) = modelica_network_members_for_stage(world, stage_id) else {
        return;
    };
    refresh_program_owner_with_network_members(world, stage_id, owner, &network_members);
}

/// Return the shared Modelica membership fact for the selected stage revision.
pub(crate) fn modelica_network_members_for_stage(
    world: &mut World,
    stage_id: AssetId<UsdStageAsset>,
) -> Option<Arc<HashSet<String>>> {
    if world
        .get_resource::<program::ModelicaNetworkMembershipCache>()
        .is_none()
    {
        warn!("[usd] Modelica network membership cache is unavailable");
        return None;
    }
    let (generation, members) = {
        let Some(stage_asset) = world
            .get_resource::<Assets<UsdStageAsset>>()
            .and_then(|assets| assets.get(stage_id))
        else {
            warn!(
                "[usd] stage asset {stage_id:?} is unavailable while resolving Modelica network membership"
            );
            return None;
        };
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            warn!(
                "[usd] canonical stage reader is unavailable while resolving Modelica network membership"
            );
            return None;
        };
        let (reader, generation) = stages.reader_for(stage_id, stage_asset);
        if let Some(members) = world
            .get_resource::<program::ModelicaNetworkMembershipCache>()
            .and_then(|cache| cache.get(stage_id, generation, None))
        {
            return Some(members);
        }
        (generation, program::modelica_network_member_paths(&reader))
    };
    Some(
        world
            .resource_mut::<program::ModelicaNetworkMembershipCache>()
            .insert(stage_id, generation, None, members),
    )
}

/// Refresh one owner using a membership snapshot shared by a projection batch.
///
/// Network membership is a stage-wide fact. Initial scene projection can admit
/// many owners in one update, so the caller must share that fact instead of
/// rescanning the composed stage once per owner.
pub(crate) fn refresh_program_owner_with_network_members(
    world: &mut World,
    stage_id: AssetId<UsdStageAsset>,
    owner: Entity,
    network_members: &HashSet<String>,
) {
    let preview_only = lunco_usd_bevy_scene::is_preview_only_entity(world, owner);
    if preview_only {
        return;
    }
    let Some(owner_path) = world
        .get::<UsdPrimPath>(owner)
        .map(|path| path.path.clone())
    else {
        warn!("[usd] projected owner {owner:?} has no USD path while refreshing authored programs");
        return;
    };
    let Some(stage_asset) = world
        .get_resource::<Assets<UsdStageAsset>>()
        .and_then(|assets| assets.get(stage_id))
    else {
        warn!("[usd] stage asset {stage_id:?} is unavailable for projected owner {owner_path}");
        return;
    };
    let Some(stages) = world.get_non_send::<CanonicalStages>() else {
        warn!("[usd] canonical stage reader is unavailable for projected owner {owner_path}");
        return;
    };
    let instance = world.get::<UsdInstanceProjection>(owner);
    let prepared = {
        let (view, _) = stages.reader_for_entity(stage_id, stage_asset, instance);
        let owner = SdfPath::new(&owner_path).expect("projected USD path is valid");
        let owner_children = UsdRead::children(&view, &owner);
        resolve_program_owner(&view, &owner, &owner_children, network_members)
    };
    apply_program_owner_projection(world, owner, stage_id, prepared);
}

/// Program attributes and implementation resolved from one composed owner view.
pub(crate) struct PreparedProgramOwner {
    program_path: String,
    resolved: program::ResolvedProgram,
    params: HashMap<String, f64>,
}

/// Resolve generic program children using an already-read owner child list.
///
/// Initial authored-runtime projection shares this child snapshot with control
/// surface discovery so both consumers reuse the same owner traversal.
pub(crate) fn resolve_program_owner(
    view: &dyn UsdReadObject,
    owner: &SdfPath,
    owner_children: &[SdfPath],
    network_members: &HashSet<String>,
) -> Option<PreparedProgramOwner> {
    let mut candidates: Vec<_> = owner_children
        .iter()
        .filter(|child| view.is_active(child))
        .filter(|child| view.has_api_schema(child, "LunCoProgramAPI"))
        .collect();
    if view.type_name(owner).as_deref() != Some("Scope")
        && view.has_api_schema(owner, "LunCoProgramAPI")
    {
        candidates.push(owner);
    }

    let mut programs = Vec::new();
    for child in candidates {
        if network_members.contains(child.as_str()) {
            continue;
        }
        let resolved = match program::resolve_program(view, child) {
            Ok(resolved) if program::is_generic_program_backend(resolved.backend) => resolved,
            Ok(_) => continue,
            Err(issue) => {
                warn!(
                    "[usd] program {} is unresolved at {}: {}",
                    child.as_str(),
                    issue.property,
                    issue.message
                );
                continue;
            }
        };
        let params = view
            .attr_names(child)
            .iter()
            .filter_map(|name| {
                let key = name.strip_prefix("lunco:param:")?;
                Some((key.to_string(), view.real(child, name)?))
            })
            .collect::<HashMap<_, _>>();
        programs.push((child.to_string(), resolved, params));
    }
    if programs.len() > 1 {
        warn!(
            "[usd] {} has {} generic executable program children; none was attached",
            owner.as_str(),
            programs.len()
        );
        return None;
    }
    programs
        .into_iter()
        .next()
        .map(|(program_path, resolved, params)| PreparedProgramOwner {
            program_path,
            resolved,
            params,
        })
}

/// Commit one owner's prepared generic program resolution to ECS.
pub(crate) fn apply_program_owner_projection(
    world: &mut World,
    owner: Entity,
    stage_id: AssetId<UsdStageAsset>,
    prepared: Option<PreparedProgramOwner>,
) {
    let Some(prepared) = prepared else {
        let mut entity = world.entity_mut(owner);
        entity
            .remove::<lunco_core::ScriptParams>()
            .remove::<lunco_core::ScenarioProgramPrim>();
        drop(entity);
        lunco_usd_bevy_core::program::apply_program_resolution(world, owner, stage_id, None);
        return;
    };

    let mut entity = world.entity_mut(owner);
    if prepared.params.is_empty() {
        entity.remove::<lunco_core::ScriptParams>();
    } else {
        entity.insert(lunco_core::ScriptParams(prepared.params));
    }
    entity.insert(lunco_core::ScenarioProgramPrim(prepared.program_path));
    drop(entity);
    lunco_usd_bevy_core::program::apply_program_resolution(
        world,
        owner,
        stage_id,
        Some(prepared.resolved),
    );
}
