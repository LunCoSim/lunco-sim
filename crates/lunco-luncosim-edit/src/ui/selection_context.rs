//! Read-only, session-scoped selection context for the Assembly Editor.
//!
//! The editor selection is stored as canonical USD paths in
//! [`super::EditorSessionSelections`]. ECS entities are only the live
//! projection used by existing Inspector and gizmo panels. This provider
//! exposes the canonical identity and the current composed USD facts to Rhai,
//! HTTP, MCP, and other API consumers without making an entity id or a display
//! name part of the authoring contract.

use std::collections::HashSet;

use bevy::ecs::query::QueryState;
use bevy::prelude::*;
use lunco_api::queries::ApiQueryProvider;
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_core::markers::Callsign;
use lunco_core::{entity_display_name, CatalogEntryId};
use lunco_usd::ui::viewport::{UsdPreviewId, UsdViewportState};
use lunco_usd_bevy::{CanonicalStages, SdfPath, UsdPrimPath, UsdRead};

use super::EditorSessionSelections;

/// Read the exact selection of one open USD Editor preview.
pub(crate) struct InspectUsdSelectionProvider;

#[derive(Clone)]
struct SelectionEntity {
    path: String,
    display_name: String,
    assembly_path: Option<String>,
}

fn preview_id(params: &serde_json::Value) -> Result<Option<UsdPreviewId>, ApiResponse> {
    let Some(value) = params.get("preview") else {
        return Ok(None);
    };
    let id = value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .ok_or_else(|| {
            ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "InspectUsdSelection: `preview` must be a u64",
            )
        })?;
    Ok(Some(UsdPreviewId(id)))
}

fn parent_path(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => path[..index].to_string(),
    }
}

fn entity_in_preview(
    world: &World,
    entity: Entity,
    root: Entity,
    parents: &mut QueryState<&ChildOf>,
) -> bool {
    if entity == root {
        return true;
    }
    let mut current = entity;
    while let Ok(parent) = parents.get(world, current) {
        current = parent.parent();
        if current == root {
            return true;
        }
    }
    false
}

fn assembly_path<R: UsdRead>(view: &R, path: &str) -> Option<String> {
    let mut candidate = path.to_string();
    loop {
        if let Ok(sdf) = SdfPath::new(&candidate) {
            if view
                .kind(&sdf)
                .is_some_and(|kind| kind.eq_ignore_ascii_case("assembly"))
            {
                return Some(candidate);
            }
        }
        if candidate == "/" {
            break;
        }
        candidate = parent_path(&candidate);
    }
    None
}

fn enrich_selection(
    view: &impl UsdRead,
    mut selection: SelectionEntity,
) -> Option<SelectionEntity> {
    let sdf = SdfPath::new(&selection.path).ok()?;
    view.type_name(&sdf)?;
    selection.assembly_path = assembly_path(view, &selection.path);
    Some(selection)
}

fn selection_item(
    view: &impl UsdRead,
    selection: &SelectionEntity,
    doc: lunco_doc::DocumentId,
    preview: UsdPreviewId,
    edit_target: &str,
    variant_sets: bool,
) -> Option<serde_json::Value> {
    let sdf = SdfPath::new(&selection.path).ok()?;
    let type_name = view.type_name(&sdf)?;
    let kind = view.kind(&sdf);

    // These are the existing typed public edit surfaces. The path is always
    // supplied separately by the caller; no operation is addressed by a
    // display name or an inferred entity.
    let mut supported_operations = vec![
        serde_json::json!({
            "command": "SelectUsdPrim",
            "rhai": "assembly_ui::select_prim",
        }),
        serde_json::json!({
            "command": "QueryUsdPrim",
            "rhai": "assembly_edit::inspect",
        }),
        serde_json::json!({
            "command": "ApplyUsdOps",
            "rhai": "assembly_edit::transform",
        }),
        serde_json::json!({
            "command": "ApplyUsdOp",
            "rhai": "assembly_edit::attribute",
        }),
    ];
    if variant_sets {
        supported_operations.push(serde_json::json!({
            "command": "ApplyUsdOp",
            "rhai": "assembly_edit::variant",
        }));
    }

    Some(serde_json::json!({
        "identity": {
            "preview": preview.0,
            "doc": doc,
            "path": selection.path,
        },
        "path": selection.path,
        "name": selection.display_name,
        "type_name": type_name,
        "kind": kind,
        "parent_path": parent_path(&selection.path),
        "assembly_path": selection.assembly_path,
        "edit_target": edit_target,
        "supported_operations": supported_operations,
    }))
}

impl ApiQueryProvider for InspectUsdSelectionProvider {
    fn name(&self) -> &'static str {
        "InspectUsdSelection"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(viewport) = world.get_resource::<UsdViewportState>() else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "InspectUsdSelection requires the Assembly Editor viewport",
            );
        };
        let requested_preview = match preview_id(params) {
            Ok(preview) => preview,
            Err(error) => return error,
        };
        let preview = requested_preview.or_else(|| viewport.focused_preview_id());

        let Some(preview) = preview else {
            return ApiResponse::ok(serde_json::json!({
                "preview": serde_json::Value::Null,
                "doc": serde_json::Value::Null,
                "edit_target": serde_json::Value::Null,
                "focused": false,
                "selection_state": "no_preview",
                "selection_mode": "none",
                "requires_single_target": true,
                "selected": [],
                "primary": serde_json::Value::Null,
                "inspector_target": serde_json::Value::Null,
                "stale_selection_count": 0,
                "ambiguous_paths": [],
            }));
        };
        let Some(session) = viewport.session(preview) else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("InspectUsdSelection: preview {} is not open", preview.0),
            );
        };
        let doc = session.doc();
        let edit_target = session.edit_target().as_str().to_string();
        let stage_id = session.stage_handle().id();
        let scene_root = session.scene_root();
        let focused = viewport.focused_preview_id() == Some(preview);

        // Copy the identity source before constructing queries. The focused
        // lease uses the shared selection projection; an unfocused lease uses
        // its canonical path cache and is never inferred from the live scene.
        let (selected_paths, target_path) = if focused {
            let Some(selected) = world.get_resource::<lunco_scene_commands::SelectedEntities>()
            else {
                return ApiResponse::error(
                    ApiErrorCode::InternalError,
                    "InspectUsdSelection: SelectedEntities resource is not present",
                );
            };
            (SelectedPaths::Entities(selected.entities.clone()), None)
        } else {
            let Some(sessions) = world.get_resource::<EditorSessionSelections>() else {
                return ApiResponse::error(
                    ApiErrorCode::InternalError,
                    "InspectUsdSelection: editor selection state is not present",
                );
            };
            let paths = sessions.sessions.get(&preview).cloned().unwrap_or_default();
            (SelectedPaths::Paths(paths.paths), paths.target_path)
        };

        let Some(inspector_target) = world.get_resource::<crate::InspectorTarget>() else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "InspectUsdSelection: InspectorTarget resource is not present",
            );
        };

        let query = QueryState::<(
            Entity,
            &UsdPrimPath,
            Option<&Name>,
            Option<&Callsign>,
            Option<&CatalogEntryId>,
        )>::try_new(world);
        let Some(mut q_paths) = query else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "InspectUsdSelection: USD prim query is unavailable",
            );
        };
        let Some(mut q_parents) = QueryState::<&ChildOf>::try_new(world) else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "InspectUsdSelection: hierarchy query is unavailable",
            );
        };

        let (paths, mut stale_selection_count) =
            selected_paths.resolve(world, stage_id, scene_root, &mut q_paths, &mut q_parents);
        let (target_path, mut stale_target) = if focused {
            let target = inspector_target.part.and_then(|entity| {
                q_paths.get(world, entity).ok().and_then(|(_, prim, ..)| {
                    (prim.stage_handle.id() == stage_id
                        && entity_in_preview(world, entity, scene_root, &mut q_parents))
                    .then(|| prim.path.clone())
                })
            });
            let stale = inspector_target.part.is_some() && target.is_none();
            (target, stale)
        } else {
            let stale = target_path.as_ref().is_some_and(|path| {
                !q_paths.iter(world).any(|(entity, prim, ..)| {
                    prim.stage_handle.id() == stage_id
                        && prim.path == *path
                        && entity_in_preview(world, entity, scene_root, &mut q_parents)
                })
            });
            (target_path, stale)
        };

        // Resolve path identity against the current preview projection before
        // borrowing the non-Send canonical stage. This also makes duplicate
        // prim projections visible instead of letting a `find` choose one.
        let mut entities_by_path = std::collections::HashMap::<String, Vec<SelectionEntity>>::new();
        for (entity, prim, name, callsign, catalog) in q_paths.iter(world) {
            if prim.stage_handle.id() != stage_id
                || !entity_in_preview(world, entity, scene_root, &mut q_parents)
            {
                continue;
            }
            entities_by_path
                .entry(prim.path.clone())
                .or_default()
                .push(SelectionEntity {
                    path: prim.path.clone(),
                    display_name: entity_display_name(name, callsign, catalog),
                    assembly_path: None,
                });
        }

        let mut ambiguous_paths = Vec::new();
        if let Some(path) = target_path.as_ref() {
            match entities_by_path.get(path).map(Vec::len) {
                Some(1) => {}
                Some(count) if count > 1 => {
                    ambiguous_paths.push(path.clone());
                    stale_target = true;
                }
                _ => stale_target = true,
            }
        }

        if paths.is_empty() && !stale_target {
            return ApiResponse::ok(serde_json::json!({
                "preview": preview.0,
                "doc": doc,
                "edit_target": edit_target,
                "focused": focused,
                "selection_state": "no_selection",
                "selection_mode": "none",
                "requires_single_target": true,
                "selected": [],
                "primary": serde_json::Value::Null,
                "inspector_target": serde_json::Value::Null,
                "stale_selection_count": stale_selection_count,
                "ambiguous_paths": [],
            }));
        }

        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "InspectUsdSelection: canonical USD stages are unavailable",
            );
        };
        let Some(canonical) = stages.get(stage_id) else {
            return ApiResponse::error(
                ApiErrorCode::CommandRejected,
                format!(
                    "InspectUsdSelection: preview {} has no ready composed USD stage",
                    preview.0
                ),
            );
        };
        let view = canonical.view();
        let stage = canonical.stage();

        let mut selected = Vec::new();
        for path in &paths {
            let matches = entities_by_path.get(path).cloned().unwrap_or_default();
            if matches.len() != 1 {
                if matches.len() > 1 {
                    ambiguous_paths.push(path.clone());
                } else {
                    stale_selection_count += 1;
                }
                continue;
            }
            let item = matches.into_iter().next().expect("one match");
            let Some(item) = enrich_selection(&view, item) else {
                stale_selection_count += 1;
                continue;
            };
            let sdf = SdfPath::new(&item.path).expect("enriched selection has a valid path");
            let variant_sets = stage
                .prim(sdf.as_str())
                .variant_sets()
                .get_all_variant_selections()
                .is_ok_and(|sets| !sets.is_empty());
            if let Some(item) =
                selection_item(&view, &item, doc, preview, &edit_target, variant_sets)
            {
                selected.push(item);
            } else {
                stale_selection_count += 1;
            }
        }

        let inspector_item = target_path.as_deref().and_then(|path| {
            let matches = entities_by_path.get(path).cloned().unwrap_or_default();
            if matches.len() != 1 {
                return None;
            }
            let item = enrich_selection(&view, matches[0].clone())?;
            let sdf = SdfPath::new(path).expect("enriched selection has a valid path");
            let variant_sets = stage
                .prim(sdf.as_str())
                .variant_sets()
                .get_all_variant_selections()
                .is_ok_and(|sets| !sets.is_empty());
            selection_item(&view, &item, doc, preview, &edit_target, variant_sets)
        });
        if target_path.is_some() && inspector_item.is_none() {
            stale_target = true;
        }

        ambiguous_paths.sort();
        ambiguous_paths.dedup();

        let selection_mode = match selected.len() {
            0 => "no_selection",
            1 => "single",
            _ => "multiple",
        };
        let primary = selected.last().cloned().unwrap_or(serde_json::Value::Null);
        ApiResponse::ok(serde_json::json!({
            "preview": preview.0,
            "doc": doc,
            "edit_target": edit_target,
            "focused": focused,
            "selection_state": selection_mode,
            "selection_mode": selection_mode,
            "requires_single_target": selected.len() != 1,
            "selected": selected,
            "primary": primary,
            "inspector_target": inspector_item.unwrap_or(serde_json::Value::Null),
            "stale_selection_count": stale_selection_count + usize::from(stale_target),
            "ambiguous_paths": ambiguous_paths,
        }))
    }
}

enum SelectedPaths {
    Entities(Vec<Entity>),
    Paths(Vec<String>),
}

impl SelectedPaths {
    fn resolve(
        self,
        world: &World,
        stage_id: AssetId<lunco_usd_bevy::UsdStageAsset>,
        scene_root: Entity,
        q_paths: &mut QueryState<(
            Entity,
            &UsdPrimPath,
            Option<&Name>,
            Option<&Callsign>,
            Option<&CatalogEntryId>,
        )>,
        q_parents: &mut QueryState<&ChildOf>,
    ) -> (Vec<String>, usize) {
        let candidates = match self {
            Self::Entities(entities) => {
                let mut candidates = Vec::with_capacity(entities.len());
                let mut stale = 0;
                for entity in entities {
                    let Some(path) = q_paths.get(world, entity).ok().and_then(|(_, prim, ..)| {
                        (prim.stage_handle.id() == stage_id
                            && entity_in_preview(world, entity, scene_root, q_parents))
                        .then(|| prim.path.clone())
                    }) else {
                        stale += 1;
                        continue;
                    };
                    candidates.push(path);
                }
                (candidates, stale)
            }
            Self::Paths(paths) => (paths, 0),
        };
        let mut seen = HashSet::new();
        let mut paths = Vec::new();
        let (candidates, mut stale) = candidates;
        for path in candidates {
            if path.is_empty() {
                stale += 1;
                continue;
            }
            if !seen.insert(path.clone()) {
                continue;
            }
            let valid = q_paths.iter(world).any(|(entity, prim, ..)| {
                prim.stage_handle.id() == stage_id
                    && prim.path == path
                    && entity_in_preview(world, entity, scene_root, q_parents)
            });
            if valid {
                paths.push(path);
            } else {
                stale += 1;
            }
        }
        (paths, stale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_derived_without_name_matching() {
        assert_eq!(parent_path("/Rover/Wheel_FL"), "/Rover");
        assert_eq!(parent_path("/Rover"), "/");
    }

    #[test]
    fn no_focused_preview_is_explicit_empty_context() {
        let mut world = World::new();
        world.insert_resource(UsdViewportState::default());
        let response = InspectUsdSelectionProvider.execute(&world, &serde_json::json!({}));
        let ApiResponse::Ok { data: Some(data) } = response else {
            panic!("expected an empty selection context");
        };
        assert_eq!(data["selection_state"], "no_preview");
        assert!(data["selected"].as_array().unwrap().is_empty());
    }
}
