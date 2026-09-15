//! Runtime attachment of generic USD programs to projected Bevy owners.
//!
//! Program resolution is a USD runtime concern rather than a visual
//! projection concern. Keeping the live edit and structural refresh bridge
//! here lets the visual crate project scene data without owning executable
//! policy.

use bevy::asset::AssetId;
use bevy::prelude::{warn, Entity, World};
use openusd::sdf::Path as SdfPath;

use lunco_usd_bevy_core::{canonical::CanonicalStages, program, UsdRead, UsdStageAsset};
use lunco_usd_bevy_scene::UsdPrimPath;

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
    let Some(owner_path) = world
        .get::<UsdPrimPath>(owner)
        .map(|path| path.path.clone())
    else {
        return;
    };
    let Some((program_path, resolved, params)) = ({
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return;
        };
        let Some(stage) = stages.get(stage_id) else {
            return;
        };
        let view = stage.view();
        let owner = SdfPath::new(&owner_path).expect("projected USD path is valid");
        let network_members = program::modelica_network_member_paths(&view);
        let mut candidates: Vec<SdfPath> = UsdRead::children(&view, &owner)
            .into_iter()
            .filter(|child| UsdRead::is_active(&view, child))
            .filter(|child| UsdRead::has_api_schema(&view, child, "LunCoProgramAPI"))
            .collect();
        if UsdRead::type_name(&view, &owner).as_deref() != Some("Scope")
            && UsdRead::has_api_schema(&view, &owner, "LunCoProgramAPI")
        {
            candidates.push(owner.clone());
        }

        let mut programs = Vec::new();
        for child in candidates {
            if network_members.contains(child.as_str()) {
                continue;
            }
            let resolved = match program::resolve_program(&view, &child) {
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
            let params = UsdRead::attr_names(&view, &child)
                .iter()
                .filter_map(|name| {
                    let key = name.strip_prefix("lunco:param:")?;
                    Some((key.to_string(), UsdRead::real(&view, &child, name)?))
                })
                .collect::<std::collections::HashMap<_, _>>();
            programs.push((child.to_string(), resolved, params));
        }
        if programs.len() > 1 {
            warn!(
                "[usd] {} has {} generic executable program children; none was attached",
                owner_path,
                programs.len()
            );
            None
        } else {
            programs.into_iter().next()
        }
    }) else {
        let mut entity = world.entity_mut(owner);
        entity
            .remove::<lunco_core::ScriptParams>()
            .remove::<lunco_core::ScenarioProgramPrim>();
        drop(entity);
        lunco_usd_bevy_core::program::apply_program_resolution(world, owner, stage_id, None);
        return;
    };

    let mut entity = world.entity_mut(owner);
    if params.is_empty() {
        entity.remove::<lunco_core::ScriptParams>();
    } else {
        entity.insert(lunco_core::ScriptParams(params));
    }
    entity.insert(lunco_core::ScenarioProgramPrim(program_path));
    drop(entity);
    lunco_usd_bevy_core::program::apply_program_resolution(world, owner, stage_id, Some(resolved));
}
