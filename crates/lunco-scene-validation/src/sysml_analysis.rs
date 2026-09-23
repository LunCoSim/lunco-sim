//! Generic, policy-neutral SysML semantic snapshots for Rhai tools.
//!
//! The source resolver and parser provide the typed model. This adapter
//! projects requested tables and exact name selections without interpreting
//! requirements or verification policy; Rhai owns those decisions.

use bevy::prelude::*;
use lunco_api::{ApiQueryError, ApiQueryProvider, ApiQueryRegistry, ApiQueryResult};
use lunco_api_core::ApiErrorCode;
use lunco_hooks::HookValue;
use lunco_sysml_ast::lint_facts::{SysmlFactPage, SysmlFactSelection, SysmlFactTable};
use lunco_sysml_ast::{SysmlElementHandle, SysmlFeatureHandle};
use std::collections::{BTreeSet, HashSet};

use crate::validate::validate_sysml_reference;

/// Register the generic `AnalyzeSysml` source query.
pub fn register(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(AnalyzeSysmlProvider);
}

struct AnalyzeSysmlProvider;

impl ApiQueryProvider for AnalyzeSysmlProvider {
    fn name(&self) -> &'static str {
        "AnalyzeSysml"
    }

    fn execute(&self, world: &World, params: &HookValue) -> ApiQueryResult {
        let Some(path) = lunco_api::api_param_str(params, "path") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "AnalyzeSysml requires params.path (string): a filesystem path or twin:// URI",
            ));
        };
        // Semantic fact discovery is policy-neutral. Structural quality is
        // reported by ValidateSysml; this query only fails for source
        // resolution/parsing diagnostics so other Rhai policies can inspect
        // the same resolved facts independently.
        let report = validate_sysml_reference(world, path, false);
        let selection = SysmlFactSelection {
            tables: parse_tables(params)?,
            page: parse_page(params)?,
            attribute_names: parse_attribute_names(params)?,
            attribute_handles: parse_feature_handle_selection(params, "attribute_handles")?,
            attribute_owners: parse_name_selection(params, "attribute_owners")?,
            attribute_owner_handles: parse_handle_selection(params, "attribute_owner_handles")?,
            attribute_string_values: parse_name_selection(params, "attribute_string_values")?,
            requirement_names: parse_name_selection(params, "requirement_names")?,
            verification_names: parse_name_selection(params, "verification_names")?,
            reference_target_names: parse_name_selection(params, "reference_target_names")?,
            reference_target_handles: parse_handle_selection(params, "reference_target_handles")?,
            reference_from_owners: parse_name_selection(params, "reference_from_owners")?,
            reference_from_owner_handles: parse_handle_selection(
                params,
                "reference_from_owner_handles",
            )?,
            reference_from_features: parse_name_selection(params, "reference_from_features")?,
        };
        let facts = report.sysml_analysis.as_deref().map_or_else(
            || HookValue::Unit,
            |analysis| lunco_sysml_ast::lint_facts::selected_sysml_facts(analysis, &selection),
        );

        // `HookValue` is the language-neutral in-process ABI. No JSON staging
        // is used: numeric literals, identity, collections and source spans
        // arrive as typed Rhai values for policy-level selection.
        Ok(Some(HookValue::map([
            ("path", HookValue::str(report.path)),
            ("kind", HookValue::str(report.kind)),
            ("ok", HookValue::Bool(report.ok)),
            (
                "errors",
                HookValue::Array(report.errors.into_iter().map(HookValue::str).collect()),
            ),
            (
                "warnings",
                HookValue::Array(report.warnings.into_iter().map(HookValue::str).collect()),
            ),
            ("analysis", facts),
        ])))
    }
}

fn parse_page(params: &HookValue) -> Result<Option<SysmlFactPage>, ApiQueryError> {
    let offset = match params.get("offset") {
        None => 0,
        Some(HookValue::Int(value)) if *value >= 0 => usize::try_from(*value).map_err(|_| {
            ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "AnalyzeSysml: `offset` is too large for this platform",
            )
        })?,
        Some(_) => {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "AnalyzeSysml: `offset` must be a non-negative integer",
            ));
        }
    };
    let limit = match params.get("limit") {
        None => None,
        Some(HookValue::Int(value)) if *value > 0 => {
            Some(usize::try_from(*value).map_err(|_| {
                ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "AnalyzeSysml: `limit` is too large for this platform",
                )
            })?)
        }
        Some(_) => {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "AnalyzeSysml: `limit` must be a positive integer",
            ));
        }
    };
    match limit {
        Some(limit) => Ok(Some(SysmlFactPage { offset, limit })),
        None if offset == 0 => Ok(None),
        None => Err(ApiQueryError::new(
            ApiErrorCode::DeserializationError,
            "AnalyzeSysml: `offset` requires a positive `limit`",
        )),
    }
}

fn parse_tables(params: &HookValue) -> Result<Option<BTreeSet<SysmlFactTable>>, ApiQueryError> {
    let Some(value) = params.get("tables") else {
        return Ok(None);
    };
    let HookValue::Array(values) = value else {
        return Err(ApiQueryError::new(
            ApiErrorCode::DeserializationError,
            "AnalyzeSysml: `tables` must be an array of SysML fact-table names",
        ));
    };
    let mut tables = BTreeSet::new();
    for value in values {
        let HookValue::Str(name) = value else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "AnalyzeSysml: `tables` must contain only strings",
            ));
        };
        let Some(table) = SysmlFactTable::parse(name) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!(
                    "AnalyzeSysml: unknown table `{name}`; supported: elements, references, relationships, constraints, attributes, requirements, verifications, diagnostics"
                ),
            ));
        };
        tables.insert(table);
    }
    Ok(Some(tables))
}

fn parse_attribute_names(params: &HookValue) -> Result<Option<BTreeSet<String>>, ApiQueryError> {
    parse_name_selection(params, "attribute_names")
}

fn parse_name_selection(
    params: &HookValue,
    key: &'static str,
) -> Result<Option<BTreeSet<String>>, ApiQueryError> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    let HookValue::Array(values) = value else {
        return Err(ApiQueryError::new(
            ApiErrorCode::DeserializationError,
            format!("AnalyzeSysml: `{key}` must be an array of qualified or local names"),
        ));
    };
    let mut names = BTreeSet::new();
    for value in values {
        let HookValue::Str(name) = value else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!("AnalyzeSysml: `{key}` must contain only strings"),
            ));
        };
        names.insert(name.clone());
    }
    Ok(Some(names))
}

fn parse_handle_selection(
    params: &HookValue,
    key: &'static str,
) -> Result<Option<HashSet<SysmlElementHandle>>, ApiQueryError> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    let HookValue::Array(values) = value else {
        return Err(ApiQueryError::new(
            ApiErrorCode::DeserializationError,
            format!("AnalyzeSysml: `{key}` must be an array of SysML element handles"),
        ));
    };
    let mut handles = HashSet::with_capacity(values.len());
    for value in values {
        let handle = parse_element_handle(value).ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!("AnalyzeSysml: `{key}` contains an invalid SysML element handle"),
            )
        })?;
        handles.insert(handle);
    }
    Ok(Some(handles))
}

fn parse_feature_handle_selection(
    params: &HookValue,
    key: &'static str,
) -> Result<Option<HashSet<SysmlFeatureHandle>>, ApiQueryError> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    let HookValue::Array(values) = value else {
        return Err(ApiQueryError::new(
            ApiErrorCode::DeserializationError,
            format!("AnalyzeSysml: `{key}` must be an array of SysML feature handles"),
        ));
    };
    let mut handles = HashSet::with_capacity(values.len());
    for value in values {
        let element = value
            .get("element")
            .and_then(parse_element_handle)
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    format!("AnalyzeSysml: `{key}` contains an invalid SysML feature handle"),
                )
            })?;
        handles.insert(SysmlFeatureHandle { element });
    }
    Ok(Some(handles))
}

fn parse_element_handle(value: &HookValue) -> Option<SysmlElementHandle> {
    let source_revision = parse_unsigned(value.get("source_revision")?)?;
    let source_fingerprint = parse_unsigned(value.get("source_fingerprint")?)?;
    let element_id = u32::try_from(value.get("element_id")?.as_u64()?).ok()?;
    Some(SysmlElementHandle {
        source_revision,
        source_fingerprint,
        element_id,
    })
}

fn parse_unsigned(value: &HookValue) -> Option<u64> {
    match value {
        HookValue::UInt(value) => Some(*value),
        _ => None,
    }
}
