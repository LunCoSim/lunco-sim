//! Initial and incremental projection of USD billboard facts.

use bevy::prelude::*;
use lunco_usd_bevy_scene::billboard::UsdBillboard;
use lunco_usd_bevy_scene::prim_index::UsdPrimIndex;
use lunco_usd_bevy_stage::read::{UsdReadObject, read_authored_bool_strict};
use lunco_usd_bevy_stage::{UsdStageAsset, canonical::CanonicalStages};
use openusd::sdf::{Path as SdfPath, Value};

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

pub(crate) fn refresh_prims(
    world: &mut World,
    stage: AssetId<UsdStageAsset>,
    prims: &lunco_usd_bevy_core::live_edit::UsdPropertyChanges,
) {
    let updates = {
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return;
        };
        let Some(canonical) = stages.get(stage) else {
            return;
        };
        let Some(index) = world.get_resource::<UsdPrimIndex>() else {
            return;
        };
        let view = canonical.view();
        prims
            .keys()
            .filter_map(|path| {
                let mut entities = index.in_stage(stage, path.as_str()).to_vec();
                if entities.is_empty() {
                    return None;
                }
                entities.sort_unstable_by_key(|entity| entity.to_bits());
                Some((path, entities, read_billboard(&view, path)))
            })
            .collect::<Vec<_>>()
    };
    for (path, entities, result) in updates {
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
        for entity in entities {
            let Ok(mut entity) = world.get_entity_mut(entity) else {
                continue;
            };
            match &result {
                Ok(Some(billboard)) => {
                    entity.insert(billboard.clone());
                }
                Ok(None) | Err(_) => {
                    entity.remove::<UsdBillboard>();
                }
            }
        }
    }
}
