//! Shared document diagnostics query and API projection.

use bevy::prelude::*;
use lunco_api_core::{ApiErrorCode, ApiQueryParameterSchema, ApiQuerySchema, ApiValue, api_value};
use lunco_doc::{
    CompileState, Diagnostic, DiagnosticSourceReport, DiagnosticSourceState, DocumentId,
};
use lunco_doc_bevy::DocumentDiagnostics;

use crate::queries::{
    ApiQueryError, ApiQueryProvider, ApiQueryResult, api_param_str, api_param_u64,
};

/// Read all current diagnostics for one document across its installed
/// compile, lint, parser, and semantic-analysis producers.
pub struct GetDiagnosticsProvider;

impl ApiQueryProvider for GetDiagnosticsProvider {
    fn name(&self) -> &'static str {
        "GetDiagnostics"
    }

    fn schema(&self) -> ApiQuerySchema {
        ApiQuerySchema {
            name: self.name().to_owned(),
            description: Some(
                "Read current diagnostics from an open document or an explicit lint scope. "
                    .to_owned(),
            ),
            parameters: Some(vec![
                ApiQueryParameterSchema {
                    name: "doc_id".to_owned(),
                    type_name: "u64".to_owned(),
                    required: false,
                    description: "Open document identity from ListOpenDocuments or its open command response; 0 selects the active document when Workspace is installed".to_owned(),
                    allowed_values: None,
                },
                ApiQueryParameterSchema {
                    name: "scope".to_owned(),
                    type_name: "string".to_owned(),
                    required: false,
                    description: "Scene lint scope: loaded_stages or twin".to_owned(),
                    allowed_values: Some(vec!["loaded_stages".to_owned(), "twin".to_owned()]),
                },
            ]),
            exactly_one_of: vec![vec!["doc_id".to_owned(), "scope".to_owned()]],
            response: Some(
                "Report fields: scope, doc_id, generation, revision, state, compile_state, available, complete, ok, errors, warnings, infos, hints, channels[], diagnostics[]. Each channel has id, domain, generation, revision, state, message, count. Each diagnostic has domain, source, code, severity, message, uri, line, column, end_line, end_column, start_offset, end_offset, subject, suggestion, and related[] (uri, line, column, end_line, end_column, message). Lines and columns are one-based; byte offsets are zero-based half-open ranges."
                    .to_owned(),
            ),
        }
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let raw_doc_id = match params.get("doc_id") {
            None => None,
            Some(_) => Some(api_param_u64(params, "doc_id").ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "GetDiagnostics `doc_id` must be an unsigned integer",
                )
            })?),
        };
        let scope = match params.get("scope") {
            None => None,
            Some(_) => Some(api_param_str(params, "scope").ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "GetDiagnostics `scope` must be a string",
                )
            })?),
        };
        match (raw_doc_id, scope) {
            (Some(raw_doc_id), None) => resolve_document_id(world, raw_doc_id)
                .and_then(|doc_id| document_diagnostics(world, doc_id)),
            (None, Some(scope)) => scene_diagnostics(world, scope),
            _ => Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "GetDiagnostics requires exactly one of `doc_id` or `scope`",
            )),
        }
    }
}

fn resolve_document_id(world: &World, raw_doc_id: u64) -> Result<DocumentId, ApiQueryError> {
    if raw_doc_id != 0 {
        return Ok(DocumentId::new(raw_doc_id));
    }
    let Some(workspace) = world.get_resource::<lunco_workspace::WorkspaceResource>() else {
        return Err(ApiQueryError::new(
            ApiErrorCode::CommandRejected,
            "GetDiagnostics cannot resolve `doc_id: 0` without Workspace; pass an explicit document id",
        ));
    };
    workspace.active_document.ok_or_else(|| {
        ApiQueryError::new(
            ApiErrorCode::EntityNotFound,
            "GetDiagnostics has no active document; pass an explicit document id",
        )
    })
}

fn document_diagnostics(world: &World, doc_id: DocumentId) -> ApiQueryResult {
    let raw_doc_id = doc_id.raw();
    let Some(store) = world.get_resource::<DocumentDiagnostics>() else {
        return Err(ApiQueryError::new(
            ApiErrorCode::InternalError,
            "document diagnostics store is not installed",
        ));
    };
    let report = store.get(doc_id);
    let compile_state = report.map_or(CompileState::Idle, |entry| entry.state);
    let source_reports = report
        .map(|entry| entry.sources.values().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut findings = Vec::new();
    if let Some(entry) = report {
        findings.extend(
            entry
                .diagnostics
                .iter()
                .map(|finding| diagnostic_api_value(finding, None, Some("compile"))),
        );
        for source in &source_reports {
            findings.extend(source.diagnostics.iter().map(|finding| {
                diagnostic_api_value(
                    finding,
                    Some(source.domain.as_str()),
                    Some(source.id.as_str()),
                )
            }));
        }
    }
    let count_severity = |severity| {
        findings
            .iter()
            .filter(|finding| finding.get("severity").and_then(ApiValue::as_str) == Some(severity))
            .count()
    };
    let errors = count_severity("error");
    let warnings = count_severity("warning");
    let infos = count_severity("info");
    let hints = count_severity("hint");
    let generation = source_reports
        .first()
        .map(|source| source.generation)
        .filter(|generation| {
            source_reports
                .iter()
                .all(|source| source.generation == *generation)
        });
    let pending = compile_state == CompileState::Compiling
        || source_reports
            .iter()
            .any(|source| source.state == DiagnosticSourceState::Pending);
    let failed = source_reports
        .iter()
        .any(|source| matches!(source.state, DiagnosticSourceState::Failed(_)));
    let unavailable = source_reports
        .iter()
        .any(|source| matches!(source.state, DiagnosticSourceState::Unavailable(_)));
    let available = report.is_some();
    let complete = available && !pending;
    let state = if pending {
        "pending"
    } else if failed {
        "failed"
    } else if unavailable {
        "partial"
    } else if errors > 0 || compile_state == CompileState::Error {
        "error"
    } else if available {
        "ready"
    } else {
        "unavailable"
    };
    let channels = std::iter::once(compile_channel(compile_state, report))
        .chain(source_reports.iter().map(|source| source_channel(source)))
        .collect::<Vec<_>>();

    Ok(Some(api_value!({
        "scope": "document",
        "doc_id": raw_doc_id,
        "generation": generation,
        "revision": report.map(|entry| entry.revision),
        "state": state,
        "compile_state": compile_state.as_str(),
        "available": available,
        "complete": complete,
        "ok": state == "ready" && errors == 0,
        "errors": errors,
        "warnings": warnings,
        "infos": infos,
        "hints": hints,
        "channels": channels,
        "diagnostics": findings,
    })))
}

fn scene_diagnostics(world: &World, scope: &str) -> ApiQueryResult {
    if !matches!(scope, "loaded_stages" | "twin") {
        return Err(ApiQueryError::new(
            ApiErrorCode::DeserializationError,
            "GetDiagnostics `scope` must be `loaded_stages` or `twin`",
        ));
    }
    let report = world.get_resource::<lunco_lint::LintReport>();
    let findings = report
        .map(|report| {
            report
                .findings
                .iter()
                .filter(|finding| (scope == "twin") == (finding.domain.as_deref() == Some("twin")))
                .map(|finding| diagnostic_api_value(finding, None, None))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let count_severity = |severity| {
        findings
            .iter()
            .filter(|finding| finding.get("severity").and_then(ApiValue::as_str) == Some(severity))
            .count()
    };
    let errors = count_severity("error");
    let warnings = count_severity("warning");
    let infos = count_severity("info");
    let hints = count_severity("hint");
    let scope_report = report.and_then(|report| report.scopes.get(scope));
    let scope_state = scope_report.map(|report| &report.state);
    let pending =
        scope_state.is_some_and(|state| matches!(state, lunco_lint::LintScopeState::Pending));
    let state = if scope_state.is_none() {
        "unavailable"
    } else if pending {
        "pending"
    } else if matches!(scope_state, Some(lunco_lint::LintScopeState::Failed(_))) {
        "failed"
    } else if errors > 0 {
        "error"
    } else {
        "ready"
    };
    let domain = if scope == "twin" { "twin" } else { "usd" };
    let channel = api_value!({
        "id": if scope == "twin" { "lint.twin" } else { "lint.loaded-stages" },
        "domain": domain,
        "generation": (),
        "revision": scope_report.map(|report| report.revision),
        "state": scope_report.map_or("unavailable", |report| report.state.as_str()),
        "message": scope_state.and_then(lunco_lint::LintScopeState::message).or_else(|| {
            scope_state.is_none().then_some("RunLint has not started for this scope")
        }),
        "count": findings.len(),
    });
    let complete = scope_state.is_some() && !pending;
    Ok(Some(api_value!({
        "scope": scope,
        "doc_id": (),
        "generation": (),
        "revision": scope_report.map(|report| report.revision),
        "state": state,
        "compile_state": (),
        "available": scope_report.is_some(),
        "complete": complete,
        "ok": state == "ready",
        "errors": errors,
        "warnings": warnings,
        "infos": infos,
        "hints": hints,
        "channels": [channel],
        "diagnostics": findings,
    })))
}

fn compile_channel(state: CompileState, report: Option<&lunco_doc::DocDiagnostics>) -> ApiValue {
    api_value!({
        "id": "compile",
        "domain": (),
        "generation": (),
        "revision": (),
        "state": state.as_str(),
        "message": (),
        "count": report.map_or(0, |entry| entry.diagnostics.len()),
    })
}

fn source_channel(report: &DiagnosticSourceReport) -> ApiValue {
    api_value!({
        "id": report.id.clone(),
        "domain": report.domain.clone(),
        "generation": report.generation,
        "revision": report.revision,
        "state": report.state.as_str(),
        "message": report.message.clone().or_else(|| report.state.message().map(str::to_owned)),
        "count": report.diagnostics.len(),
    })
}

/// Project the shared diagnostic contract into the typed API value ABI.
pub fn diagnostic_api_value(
    diagnostic: &Diagnostic,
    default_domain: Option<&str>,
    default_source: Option<&str>,
) -> ApiValue {
    let related = diagnostic
        .related
        .iter()
        .map(|item| {
            api_value!({
                "uri": item.uri.clone(),
                "line": item.line,
                "column": item.col,
                "end_line": item.end_line,
                "end_column": item.end_col,
                "message": item.message.clone(),
            })
        })
        .collect::<Vec<_>>();
    api_value!({
        "domain": diagnostic.domain.as_deref().or(default_domain),
        "source": diagnostic.source.as_deref().or(default_source),
        "code": diagnostic.code.clone(),
        "severity": diagnostic.severity.as_str(),
        "message": diagnostic.message.clone(),
        "uri": diagnostic.uri.clone(),
        "line": diagnostic.line,
        "column": diagnostic.col,
        "end_line": diagnostic.end_line,
        "end_column": diagnostic.end_col,
        "start_offset": diagnostic.start_offset,
        "end_offset": diagnostic.end_offset,
        "subject": diagnostic.subject.clone(),
        "suggestion": diagnostic.suggestion.clone(),
        "related": related,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_discloses_the_document_or_lint_scope_input_rule() {
        let schema = GetDiagnosticsProvider.schema();
        assert_eq!(schema.name, "GetDiagnostics");
        assert_eq!(
            schema.exactly_one_of,
            vec![vec!["doc_id".to_owned(), "scope".to_owned()]]
        );
        let parameters = schema
            .parameters
            .expect("query parameters are discoverable");
        assert_eq!(parameters.len(), 2);
        assert!(parameters.iter().all(|parameter| !parameter.required));
        assert_eq!(
            parameters[1].allowed_values.as_deref(),
            Some(["loaded_stages".to_owned(), "twin".to_owned()].as_slice())
        );
        let response = schema.response.expect("response contract is discoverable");
        assert!(response.contains("diagnostics[]"));
        assert!(response.contains("suggestion"));
        assert!(response.contains("related[]"));
    }

    #[test]
    fn active_document_alias_resolves_through_workspace() {
        let mut world = World::new();
        let doc_id = DocumentId::new(42);
        let mut workspace = lunco_workspace::WorkspaceResource::default();
        workspace.active_document = Some(doc_id);
        world.insert_resource(workspace);
        world.insert_resource(DocumentDiagnostics::default());

        let report = GetDiagnosticsProvider
            .execute(&world, &api_value!({ "doc_id": 0 }))
            .expect("active document alias resolves")
            .expect("query returns a report");
        assert_eq!(report.get("doc_id").and_then(ApiValue::as_u64), Some(42));
        assert_eq!(report.get("available"), Some(&ApiValue::Bool(false)));
    }

    #[test]
    fn active_document_alias_requires_an_active_workspace_document() {
        let mut world = World::new();
        world.insert_resource(lunco_workspace::WorkspaceResource::default());
        let error = GetDiagnosticsProvider
            .execute(&world, &api_value!({ "doc_id": 0 }))
            .expect_err("empty workspace has no active document");
        assert_eq!(error.code, ApiErrorCode::EntityNotFound);
    }
}
