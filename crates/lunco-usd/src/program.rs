//! Lowering for attaching a simulation program to a USD prim.
//!
//! A program attachment is authored topology, not an ECS marker. This module
//! owns the portable scalar contract used by the command, the Models palette,
//! Rhai, and HTTP callers. The cosimulation runtime remains the reader of the
//! resulting `LunCoProgramAPI` prim.

use crate::document::{LayerId, UsdOp};
use bevy::prelude::Reflect;

/// One scalar input declared by an attached program.
#[derive(Debug, Clone, PartialEq, Reflect, serde::Serialize, serde::Deserialize)]
pub struct ProgramInput {
    /// Leaf port name without the `inputs:` namespace.
    pub name: String,
    /// USD scalar type currently supported by the runtime port substrate.
    pub type_name: String,
    /// Explicit constant value when this input is not connected.
    pub default_value: Option<f64>,
    /// Absolute USD property path supplying this input.
    pub connection: Option<String>,
}

impl Default for ProgramInput {
    fn default() -> Self {
        Self {
            name: String::new(),
            type_name: "float".into(),
            default_value: None,
            connection: None,
        }
    }
}

/// One scalar output declared by an attached program.
#[derive(Debug, Clone, PartialEq, Reflect, serde::Serialize, serde::Deserialize)]
pub struct ProgramOutput {
    /// Leaf port name without the `outputs:` namespace.
    pub name: String,
    /// USD scalar type currently supported by the runtime port substrate.
    pub type_name: String,
    /// Absolute consumer properties receiving this output.
    pub connections: Vec<String>,
}

impl Default for ProgramOutput {
    fn default() -> Self {
        Self {
            name: String::new(),
            type_name: "float".into(),
            connections: Vec::new(),
        }
    }
}

/// Complete authored intent for attaching one source-backed program.
#[derive(Debug, Clone, PartialEq, Reflect, serde::Serialize, serde::Deserialize)]
pub struct ProgramAttachSpec {
    /// Layer receiving the program opinion.
    pub edit_target: LayerId,
    /// Existing USD prim that owns the attached program.
    pub host_path: String,
    /// New child name under `host_path`.
    pub name: String,
    /// Source asset path without USDA `@` delimiters.
    pub source_asset: String,
    /// Declared scalar inputs and their authored values or connections.
    pub inputs: Vec<ProgramInput>,
    /// Declared scalar outputs.
    pub outputs: Vec<ProgramOutput>,
    /// Whether the program is allowed to drive force/torque ports on a
    /// client-predicted body.
    pub realtime_safe: bool,
}

impl Default for ProgramAttachSpec {
    fn default() -> Self {
        Self {
            edit_target: LayerId::root(),
            host_path: String::new(),
            name: String::new(),
            source_asset: String::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            realtime_safe: false,
        }
    }
}

/// Build the primitive USD operations for [`ProgramAttachSpec`].
///
/// Validation happens before any operation is returned. Callers must apply the
/// returned vector with `apply_ops_as_change_set`; no caller may apply a subset.
pub fn program_attach_ops(spec: &ProgramAttachSpec) -> Result<Vec<UsdOp>, String> {
    validate_spec(spec)?;

    let program_path = format!("{}/{}", spec.host_path.trim_end_matches('/'), spec.name);
    let edit_target = spec.edit_target.clone();
    let mut ops = vec![
        UsdOp::AddPrim {
            edit_target: edit_target.clone(),
            parent_path: spec.host_path.clone(),
            name: spec.name.clone(),
            type_name: Some("Scope".into()),
            reference: None,
        },
        UsdOp::SetApiSchemas {
            edit_target: edit_target.clone(),
            path: program_path.clone(),
            schemas: vec!["LunCoProgramAPI".into()],
        },
        UsdOp::SetAttribute {
            edit_target: edit_target.clone(),
            path: program_path.clone(),
            name: "info:implementationSource".into(),
            type_name: "token".into(),
            value: "\"sourceAsset\"".into(),
        },
        UsdOp::SetAttribute {
            edit_target: edit_target.clone(),
            path: program_path.clone(),
            name: "info:sourceAsset".into(),
            type_name: "asset".into(),
            value: format!("@{}@", spec.source_asset),
        },
        UsdOp::SetAttribute {
            edit_target: edit_target.clone(),
            path: program_path.clone(),
            name: "lunco:program:realtimeSafe".into(),
            type_name: "bool".into(),
            value: spec.realtime_safe.to_string(),
        },
    ];

    for input in &spec.inputs {
        let attr = format!("inputs:{}", input.name);
        if let Some(source) = &input.connection {
            ops.push(UsdOp::SetConnection {
                edit_target: edit_target.clone(),
                path: program_path.clone(),
                name: attr,
                type_name: input.type_name.clone(),
                sources: vec![source.clone()],
            });
        } else if let Some(value) = input.default_value {
            ops.push(UsdOp::SetAttribute {
                edit_target: edit_target.clone(),
                path: program_path.clone(),
                name: attr,
                type_name: input.type_name.clone(),
                value: value.to_string(),
            });
        }
    }

    for output in &spec.outputs {
        ops.push(UsdOp::SetAttribute {
            edit_target: edit_target.clone(),
            path: program_path.clone(),
            name: format!("outputs:{}", output.name),
            type_name: output.type_name.clone(),
            value: "0.0".into(),
        });
        for target in &output.connections {
            let Some((target_path, target_attr)) = split_property_path(target) else {
                return Err(format!(
                    "program output `{}` has an invalid consumer property `{target}`",
                    output.name
                ));
            };
            ops.push(UsdOp::SetConnection {
                edit_target: edit_target.clone(),
                path: target_path.into(),
                name: target_attr.into(),
                type_name: output.type_name.clone(),
                sources: vec![format!("{}.outputs:{}", program_path, output.name)],
            });
        }
    }

    Ok(ops)
}

fn validate_spec(spec: &ProgramAttachSpec) -> Result<(), String> {
    if !is_absolute_prim_path(&spec.host_path) {
        return Err(format!(
            "program host path must be an absolute USD prim path: `{}`",
            spec.host_path
        ));
    }
    if !is_identifier(&spec.name) {
        return Err(format!(
            "program name is not a USD identifier: `{}`",
            spec.name
        ));
    }
    if spec.source_asset.is_empty()
        || spec.source_asset.contains('@')
        || spec.source_asset.starts_with('/')
        || spec.source_asset.split('/').any(|part| part == "..")
    {
        return Err(format!(
            "program source must be a non-empty asset path without `@`, absolute paths, or `..`: `{}`",
            spec.source_asset
        ));
    }
    if !spec.source_asset.ends_with(".mo")
        && !spec.source_asset.ends_with(".py")
        && !spec.source_asset.ends_with(".rhai")
        && !spec.source_asset.ends_with(".btxml")
        && !spec.source_asset.ends_with(".xml")
    {
        return Err(format!(
            "unsupported program source extension in `{}`",
            spec.source_asset
        ));
    }

    let mut names = std::collections::BTreeSet::new();
    for input in &spec.inputs {
        validate_port(&input.name, &input.type_name, "input")?;
        if !names.insert(format!("inputs:{}", input.name)) {
            return Err(format!("duplicate program input `{}`", input.name));
        }
        if input.default_value.is_some() == input.connection.is_some() {
            return Err(format!(
                "program input `{}` must have exactly one default value or connection",
                input.name
            ));
        }
        if let Some(source) = &input.connection {
            if !is_absolute_property_path(source) {
                return Err(format!(
                    "program input `{}` has an invalid connection source `{source}`",
                    input.name
                ));
            }
        }
    }
    for output in &spec.outputs {
        validate_port(&output.name, &output.type_name, "output")?;
        if !names.insert(format!("outputs:{}", output.name)) {
            return Err(format!("duplicate program port `{}`", output.name));
        }
        for target in &output.connections {
            let (_, attr) = split_property_path(target).ok_or_else(|| {
                format!(
                    "program output `{}` has an invalid consumer property `{target}`",
                    output.name
                )
            })?;
            if !attr.starts_with("inputs:") {
                return Err(format!(
                    "program output `{}` must connect to an inputs:* property, got `{target}`",
                    output.name
                ));
            }
        }
    }
    Ok(())
}

fn validate_port(name: &str, type_name: &str, direction: &str) -> Result<(), String> {
    if !is_identifier(name) {
        return Err(format!(
            "program {direction} port is not an identifier: `{name}`"
        ));
    }
    if type_name != "float" && type_name != "double" {
        return Err(format!(
            "program {direction} `{name}` uses unsupported scalar type `{type_name}`"
        ));
    }
    Ok(())
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn is_absolute_prim_path(value: &str) -> bool {
    value.starts_with('/') && value != "/" && !value.contains("//")
}

fn is_absolute_property_path(value: &str) -> bool {
    value.starts_with('/') && value.contains('.') && !value.contains("//")
}

fn split_property_path(value: &str) -> Option<(&str, &str)> {
    let (path, attr) = value.rsplit_once('.')?;
    (is_absolute_prim_path(path) && !attr.is_empty()).then_some((path, attr))
}
