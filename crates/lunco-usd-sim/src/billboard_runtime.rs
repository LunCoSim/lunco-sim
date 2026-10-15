//! Initial and incremental projection of USD billboard facts.

use bevy::prelude::*;
use lunco_usd_bevy_scene::{UsdPrimPath, billboard::UsdBillboard};
use lunco_usd_bevy_stage::read::{UsdReadObject, read_authored_bool_strict};
use lunco_usd_bevy_stage::{UsdStageAsset, canonical::CanonicalStages};
use openusd::sdf::{Path as SdfPath, Value};
use std::collections::HashMap;

pub(crate) fn claims_edit(_reader: &dyn UsdReadObject, _prim: &SdfPath, attribute: &str) -> bool {
    matches!(
        attribute,
        "lunco:billboard"
            | "lunco:billboard:text"
            | "lunco:billboard:offsetY"
            | "lunco:billboard:fadeEnd"
    )
}

pub(crate) fn read_billboard(
    reader: &dyn UsdReadObject,
    prim: &SdfPath,
) -> Result<Option<UsdBillboard>, (&'static str, &'static str)> {
    let enabled = read_authored_bool_strict(reader, prim, "lunco:billboard").map_err(|_| {
        (
            "billboard-attribute",
            "lunco:billboard must be an authored boolean",
        )
    })?;
    if enabled != Some(true) {
        return Ok(None);
    }
    let invalid = (
        "billboard-contract",
        "billboard attributes are malformed or outside their documented range",
    );
    let default = UsdBillboard::default();
    let template = match reader.attr_value(prim, "lunco:billboard:text") {
        Some(Value::String(value)) => value,
        Some(_) if reader.has_authored_attribute(prim, "lunco:billboard:text") => {
            return Err(invalid);
        }
        _ => default.template,
    };
    let read_real = |name: &str, default_value: f32| match reader.real_f32(prim, name) {
        Some(value) if value.is_finite() => Ok(value),
        Some(_) => Err(invalid),
        None if reader.has_authored_attribute(prim, name) => Err(invalid),
        None => Ok(default_value),
    };
    let offset_y = read_real("lunco:billboard:offsetY", default.offset_y)?;
    let fade_end = read_real("lunco:billboard:fadeEnd", default.fade_end)?;
    if fade_end <= 0.0 {
        return Err(invalid);
    }
    Ok(Some(UsdBillboard {
        template,
        offset_y,
        fade_end,
    }))
}

pub(crate) fn refresh_prims(world: &mut World, stage: AssetId<UsdStageAsset>, prims: &[SdfPath]) {
    let changed = prims
        .iter()
        .map(|path| (path.as_str(), path))
        .collect::<HashMap<_, _>>();
    let mut rows = {
        let mut query = world.query::<(Entity, &UsdPrimPath)>();
        query
            .iter(world)
            .filter_map(|(entity, prim)| {
                if prim.stage_handle.id() != stage {
                    return None;
                }
                changed
                    .get(prim.path.as_str())
                    .map(|path| (entity, (**path).clone()))
            })
            .collect::<Vec<_>>()
    };
    rows.sort_by(|left, right| {
        left.1
            .cmp(&right.1)
            .then(left.0.to_bits().cmp(&right.0.to_bits()))
    });
    let updates = {
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return;
        };
        let Some(stage) = stages.get(stage) else {
            return;
        };
        let view = stage.view();
        rows.into_iter()
            .map(|(entity, path)| {
                let value = read_billboard(&view, &path);
                (entity, path, value)
            })
            .collect::<Vec<_>>()
    };
    for (entity, path, result) in updates {
        {
            let mut diagnostics = world.resource_mut::<lunco_core::RuntimeDiagnostics>();
            diagnostics.findings.retain(|finding| {
                !(finding.producer == "usd-sim"
                    && finding.subject == path.as_str()
                    && matches!(
                        finding.code.as_str(),
                        "billboard-attribute" | "billboard-contract"
                    ))
            });
            if let Err((code, message)) = &result {
                crate::push_usd_sim_diagnostic(
                    &mut diagnostics.findings,
                    path.as_str(),
                    code,
                    *message,
                );
                warn!("USD prim {path} has invalid billboard attributes: {message}");
            }
        }
        let Ok(mut entity) = world.get_entity_mut(entity) else {
            continue;
        };
        match result {
            Ok(Some(billboard)) => {
                entity.insert(billboard);
            }
            Ok(None) | Err(_) => {
                entity.remove::<UsdBillboard>();
            }
        }
    }
}
