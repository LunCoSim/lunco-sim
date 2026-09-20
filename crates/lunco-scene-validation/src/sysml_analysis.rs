//! Generic, policy-neutral SysML semantic snapshots for Rhai tools.
//!
//! The source resolver and parser provide the typed model. This adapter
//! projects requested tables and exact name selections without interpreting
//! requirements or verification policy; Rhai owns those decisions.

use bevy::prelude::*;
use lunco_api::{ApiQueryError, ApiQueryProvider, ApiQueryRegistry, ApiQueryResult};
use lunco_api_core::ApiErrorCode;
use lunco_hooks::HookValue;
use lunco_sysml_ast::lint_facts::{SysmlFactSelection, SysmlFactTable};
use std::collections::BTreeSet;

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
            attribute_names: parse_attribute_names(params)?,
            requirement_names: parse_name_selection(params, "requirement_names")?,
            verification_names: parse_name_selection(params, "verification_names")?,
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
