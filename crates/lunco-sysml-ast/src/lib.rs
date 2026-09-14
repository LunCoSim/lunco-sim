//! Pure SysML v2 source analysis for LunCoSim.
//!
//! The crate owns the boundary between authored `.sysml`/`.kerml` text and
//! the upstream parser/semantic model. It deliberately has no Bevy, storage,
//! renderer, or document-system dependency. Callers receive serializable
//! projections rather than owning the upstream model directly, so UI, tests,
//! and Rhai can share one stable read-side contract.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use serde::{Deserialize, Serialize};
use sysml_semantics::Workspace;
use sysml_syntax::TextRange;

thread_local! {
    // `sysml_semantics::Workspace` owns rowan syntax nodes and is deliberately
    // !Send/!Sync. Keep one resolved library per parsing thread instead of
    // introducing an unsafe global or reparsing it for every edit.
    static STANDARD_LIBRARY: std::cell::RefCell<Option<Workspace>> = const { std::cell::RefCell::new(None) };
}

/// A source file admitted to a semantic workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlFile {
    /// Caller-provided logical name. It is not read from the filesystem.
    pub name: String,
    /// UTF-8 source text.
    pub text: String,
}

/// The category of a semantic diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SysmlDiagnosticKind {
    /// Parser could not read part of the source.
    Syntax,
    /// A written name did not resolve in scope.
    Name,
    /// A project root collides with a standard-library root package.
    Collision,
}

/// A normalized diagnostic with byte offsets into its source file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlDiagnostic {
    /// Logical source file containing the issue.
    pub file: String,
    /// Diagnostic category.
    pub kind: SysmlDiagnosticKind,
    /// Inclusive-start, exclusive-end byte range.
    pub start: u32,
    /// Inclusive-start, exclusive-end byte range.
    pub end: u32,
    /// Human-readable parser/resolver message.
    pub message: String,
}

/// A source-backed SysML element suitable for navigation and reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlElement {
    /// Stable index within the upstream semantic model snapshot.
    pub id: u32,
    /// Logical source file containing the declaration.
    pub file: String,
    /// Root-qualified name (`Package::Part`).
    pub qualified_name: String,
    /// Upstream metamodel kind (`PartDefinition`, `Requirement`, …).
    pub kind: String,
    /// Full declaration byte-range start.
    pub start: u32,
    /// Full declaration byte-range end.
    pub end: u32,
}

/// A successfully resolved source reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlReference {
    /// Logical source file containing the reference.
    pub file: String,
    /// Whole written-name byte-range start.
    pub start: u32,
    /// Whole written-name byte-range end.
    pub end: u32,
    /// Final name segment as written.
    pub name: String,
    /// Root-qualified target name.
    pub target: String,
}

/// A literal value written on a SysML attribute.
///
/// The semantic model keeps the authored expression text.  This projection
/// preserves that text and classifies simple literals without evaluating user
/// expressions.  Numeric text is retained so consumers can choose their own
/// lossless numeric representation at the boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlLiteral {
    /// Authored expression, without the trailing semicolon.
    pub literal: String,
    /// `integer`, `real`, `boolean`, `string`, or `expression`.
    pub kind: String,
    /// Canonical numeric text when the literal is an integer or real.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
}

/// An authored SysML attribute with its source span and owning element.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlAttribute {
    /// Qualified owner (`Package::Part`).
    pub owner: String,
    /// Attribute name.
    pub name: String,
    /// Qualified attribute name (`Package::Part::mass`).
    pub qualified_name: String,
    /// Declared type text, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    /// Authored literal, if the attribute has an initializer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<SysmlLiteral>,
    /// Logical source file.
    pub file: String,
    /// Declaration byte-range start.
    pub start: u32,
    /// Declaration byte-range end.
    pub end: u32,
}

/// A subject declared on a requirement or verification case.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlSubject {
    /// Local subject name.
    pub name: String,
    /// Declared subject type, if present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
}

/// A structured requirement declaration or usage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlRequirementRecord {
    /// Source-backed requirement element.
    pub element: SysmlElement,
    /// Documentation blocks owned by the requirement.
    pub documentation: Vec<String>,
    /// Declared subjects.
    pub subjects: Vec<SysmlSubject>,
    /// Attributes declared inside this requirement.
    pub attributes: Vec<SysmlAttribute>,
    /// Qualified or written requirements named by `verify` memberships.
    pub verifies: Vec<String>,
    /// Written satisfaction targets, when present.
    pub satisfies: Vec<String>,
    /// Written realization targets, when present.
    pub realizations: Vec<String>,
}

/// A structured verification case declaration or usage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlVerificationRecord {
    /// Source-backed verification element.
    pub element: SysmlElement,
    /// Documentation blocks owned by the verification case.
    pub documentation: Vec<String>,
    /// Declared subjects.
    pub subjects: Vec<SysmlSubject>,
    /// Requirements named by `verify` memberships.
    pub verifies: Vec<String>,
    /// Written realization targets, when present.
    pub realizations: Vec<String>,
}

/// The immutable, serializable projection of one resolved SysML workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysmlAnalysis {
    files: Vec<SysmlFile>,
    diagnostics: Vec<SysmlDiagnostic>,
    elements: Vec<SysmlElement>,
    references: Vec<SysmlReference>,
    attributes: Vec<SysmlAttribute>,
    requirements: Vec<SysmlRequirementRecord>,
    verifications: Vec<SysmlVerificationRecord>,
    source_revision: u64,
    includes_stdlib: bool,
}

impl SysmlAnalysis {
    /// Build an analysis with the embedded OMG standard library.
    pub fn from_files<I, N, T>(files: I) -> Self
    where
        I: IntoIterator<Item = (N, T)>,
        N: Into<String>,
        T: Into<String>,
    {
        Self::build(files, true, 0)
    }

    /// Build an analysis without loading the embedded standard library.
    ///
    /// This is useful for small parser-only fixtures; production Twin
    /// documents should use [`Self::from_files`] so standard SysML names
    /// resolve consistently.
    pub fn from_files_without_stdlib<I, N, T>(files: I) -> Self
    where
        I: IntoIterator<Item = (N, T)>,
        N: Into<String>,
        T: Into<String>,
    {
        Self::build(files, false, 0)
    }

    /// Build an analysis and stamp it with the caller's source generation.
    pub fn build<I, N, T>(files: I, includes_stdlib: bool, source_revision: u64) -> Self
    where
        I: IntoIterator<Item = (N, T)>,
        N: Into<String>,
        T: Into<String>,
    {
        let files: Vec<SysmlFile> = files
            .into_iter()
            .map(|(name, text)| SysmlFile {
                name: name.into(),
                text: text.into(),
            })
            .collect();
        // The official library is immutable data. Parse and resolve it once,
        // then clone the upstream semantic workspace for each source-set
        // snapshot. This keeps keystroke edits proportional to project size
        // instead of reparsing ~94 library files on every generation.
        let mut workspace = if includes_stdlib {
            standard_library_workspace()
        } else {
            Workspace::new()
        };
        let mut project_indices = Vec::with_capacity(files.len());
        for file in &files {
            project_indices.push(workspace.add_file(file.name.clone(), &file.text));
        }
        if !project_indices.is_empty() {
            workspace.resolve_reached(&project_indices);
        }

        let findings = workspace.findings(&project_indices);
        let mut diagnostics = Vec::new();
        diagnostics.extend(findings.syntax.into_iter().map(|finding| {
            diagnostic_from_finding(
                &workspace,
                finding.file,
                finding.range,
                finding.what,
                SysmlDiagnosticKind::Syntax,
            )
        }));
        diagnostics.extend(findings.names.into_iter().map(|finding| {
            diagnostic_from_finding(
                &workspace,
                finding.file,
                finding.range,
                format!("unresolved name `{}`", finding.what),
                SysmlDiagnosticKind::Name,
            )
        }));
        diagnostics.extend(findings.collisions.into_iter().map(|finding| {
            diagnostic_from_finding(
                &workspace,
                finding.file,
                finding.range,
                finding.what,
                SysmlDiagnosticKind::Collision,
            )
        }));

        let mut elements = Vec::new();
        for &file_idx in &project_indices {
            let file_name = workspace.file_name(file_idx).to_string();
            for &id in workspace.file_elements(file_idx) {
                let Some((range, _name_range)) = workspace.element_ranges(id) else {
                    continue;
                };
                elements.push(SysmlElement {
                    id: id.index() as u32,
                    file: file_name.clone(),
                    qualified_name: workspace.qualified_name_of(id),
                    kind: workspace.model().kind(id).name().to_string(),
                    start: u32::from(range.start()),
                    end: u32::from(range.end()),
                });
            }
        }

        let mut references = Vec::new();
        for reference in workspace.references() {
            if !project_indices.contains(&reference.file) {
                continue;
            }
            let source_text = workspace
                .file_parse(reference.file)
                .syntax()
                .text()
                .to_string();
            references.push(SysmlReference {
                file: workspace.file_name(reference.file).to_string(),
                start: u32::from(reference.range.start()),
                end: u32::from(reference.range.end()),
                name: source_text
                    .get(
                        usize::from(reference.name_range.start())
                            ..usize::from(reference.name_range.end()),
                    )
                    .unwrap_or_default()
                    .to_string(),
                target: workspace.qualified_name_of(reference.target),
            });
        }

        let attributes = project_attributes(&files, &elements);
        let requirements = project_requirements(&files, &elements, &attributes);
        let verifications = project_verifications(&files, &elements);

        Self {
            files,
            diagnostics,
            elements,
            references,
            attributes,
            requirements,
            verifications,
            source_revision,
            includes_stdlib,
        }
    }

    /// Files represented by this snapshot.
    pub fn files(&self) -> &[SysmlFile] {
        &self.files
    }

    /// Normalized semantic diagnostics for project files.
    pub fn diagnostics(&self) -> &[SysmlDiagnostic] {
        &self.diagnostics
    }

    /// Source-backed elements in project files.
    pub fn elements(&self) -> &[SysmlElement] {
        &self.elements
    }

    /// Successfully resolved source references in project files.
    pub fn references(&self) -> &[SysmlReference] {
        &self.references
    }

    /// Source-backed attributes with typed literal classification.
    pub fn attributes(&self) -> &[SysmlAttribute] {
        &self.attributes
    }

    /// Structured requirement definitions and usages.
    pub fn requirements(&self) -> &[SysmlRequirementRecord] {
        &self.requirements
    }

    /// Structured verification-case definitions and usages.
    pub fn verifications(&self) -> &[SysmlVerificationRecord] {
        &self.verifications
    }

    /// Source generation used to produce this snapshot.
    pub fn source_revision(&self) -> u64 {
        self.source_revision
    }

    /// Whether the embedded SysML standard library was included.
    pub fn includes_stdlib(&self) -> bool {
        self.includes_stdlib
    }

    /// Whether any parser, name, or collision diagnostic is present.
    pub fn has_errors(&self) -> bool {
        !self.diagnostics.is_empty()
    }
}

fn standard_library_workspace() -> Workspace {
    STANDARD_LIBRARY.with(|cache| {
        let mut cached = cache.borrow_mut();
        if cached.is_none() {
            let mut workspace = Workspace::new();
            for (name, text) in sysml_stdlib::FILES {
                workspace.add_file(*name, text);
            }
            workspace.resolve_all();
            *cached = Some(workspace);
        }
        cached
            .as_ref()
            .expect("standard library cache initialized")
            .clone()
    })
}

fn project_attributes(files: &[SysmlFile], elements: &[SysmlElement]) -> Vec<SysmlAttribute> {
    elements
        .iter()
        .filter(|element| element.kind == "AttributeDefinition" || element.kind == "AttributeUsage")
        .filter_map(|element| {
            let source = files
                .iter()
                .find(|file| file.name == element.file)?
                .text
                .as_str();
            let declaration = source.get(element.start as usize..element.end as usize)?;
            let rest = declaration.trim().strip_prefix("attribute")?.trim_start();
            let rest = rest
                .strip_prefix("def")
                .map(str::trim_start)
                .unwrap_or(rest);
            let name_end =
                rest.find(|c: char| c == ':' || c == '=' || c == ';' || c.is_whitespace())?;
            let name = rest[..name_end].trim();
            if name.is_empty() {
                return None;
            }
            let type_name = rest
                .split_once(':')
                .map(|(_, tail)| tail.split(['=', ';']).next().unwrap_or(tail).trim())
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            let value = rest
                .split_once('=')
                .map(|(_, tail)| tail.trim().trim_end_matches(';').trim())
                .filter(|literal| !literal.is_empty())
                .map(parse_literal);
            let owner = element
                .qualified_name
                .rsplit_once("::")
                .map(|(owner, _)| owner.to_owned())
                .unwrap_or_default();
            Some(SysmlAttribute {
                owner,
                name: name.to_owned(),
                qualified_name: element.qualified_name.clone(),
                type_name,
                value,
                file: element.file.clone(),
                start: element.start,
                end: element.end,
            })
        })
        .collect()
}

fn parse_literal(literal: &str) -> SysmlLiteral {
    let number = literal
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite());
    let kind = if literal.parse::<i64>().is_ok() {
        "integer"
    } else if number.is_some() {
        "real"
    } else if literal == "true" || literal == "false" {
        "boolean"
    } else if literal.starts_with('"') && literal.ends_with('"') {
        "string"
    } else {
        "expression"
    };
    SysmlLiteral {
        literal: literal.to_owned(),
        kind: kind.to_owned(),
        number: number.map(|_| literal.to_owned()),
    }
}

fn project_requirements(
    files: &[SysmlFile],
    elements: &[SysmlElement],
    attributes: &[SysmlAttribute],
) -> Vec<SysmlRequirementRecord> {
    elements
        .iter()
        .filter(|element| {
            element.kind == "RequirementDefinition" || element.kind == "RequirementUsage"
        })
        .filter_map(|element| {
            let source = files
                .iter()
                .find(|file| file.name == element.file)?
                .text
                .as_str();
            let block = source.get(element.start as usize..element.end as usize)?;
            // The semantic metamodel represents a `verify R;` membership as a
            // RequirementUsage as well.  It is owned by the verification case,
            // not a standalone requirement record, so keep only declarations
            // whose source actually starts with `requirement`.
            if !block.trim_start().starts_with("requirement") {
                return None;
            }
            let fields = parse_block_fields(block);
            let owned_attributes = attributes
                .iter()
                .filter(|attribute| {
                    attribute.file == element.file
                        && attribute.start >= element.start
                        && attribute.end <= element.end
                })
                .cloned()
                .collect();
            Some(SysmlRequirementRecord {
                element: element.clone(),
                documentation: fields.documentation,
                subjects: fields.subjects,
                attributes: owned_attributes,
                verifies: fields.verifies,
                satisfies: fields.satisfies,
                realizations: fields.realizations,
            })
        })
        .collect()
}

fn project_verifications(
    files: &[SysmlFile],
    elements: &[SysmlElement],
) -> Vec<SysmlVerificationRecord> {
    elements
        .iter()
        .filter(|element| {
            element.kind == "VerificationCaseDefinition" || element.kind == "VerificationCaseUsage"
        })
        .filter_map(|element| {
            let source = files
                .iter()
                .find(|file| file.name == element.file)?
                .text
                .as_str();
            let block = source.get(element.start as usize..element.end as usize)?;
            let fields = parse_block_fields(block);
            Some(SysmlVerificationRecord {
                element: element.clone(),
                documentation: fields.documentation,
                subjects: fields.subjects,
                verifies: fields.verifies,
                realizations: fields.realizations,
            })
        })
        .collect()
}

#[derive(Default)]
struct BlockFields {
    documentation: Vec<String>,
    subjects: Vec<SysmlSubject>,
    verifies: Vec<String>,
    satisfies: Vec<String>,
    realizations: Vec<String>,
}

fn parse_block_fields(block: &str) -> BlockFields {
    let mut fields = BlockFields::default();
    for line in block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("doc /*") {
            let text = rest
                .split_once("*/")
                .map(|(text, _)| text)
                .unwrap_or(rest)
                .trim();
            if !text.is_empty() {
                fields.documentation.push(text.to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("subject ") {
            let rest = rest.trim().trim_end_matches(';').trim();
            let (name, type_name) = rest
                .split_once(':')
                .map(|(name, ty)| (name.trim(), Some(ty.trim().to_owned())))
                .unwrap_or((rest, None));
            if !name.is_empty() {
                fields.subjects.push(SysmlSubject {
                    name: name.to_owned(),
                    type_name,
                });
            }
        } else if let Some(rest) = line.strip_prefix("verify ") {
            let target = rest.trim().trim_end_matches(';').trim();
            if !target.is_empty() {
                fields.verifies.push(target.to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("satisfy ") {
            let target = rest.trim().trim_end_matches(';').trim();
            if !target.is_empty() {
                fields.satisfies.push(target.to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("realize ") {
            let target = rest.trim().trim_end_matches(';').trim();
            if !target.is_empty() {
                fields.realizations.push(target.to_owned());
            }
        }
    }
    fields
}

fn diagnostic_from_finding(
    workspace: &Workspace,
    file: usize,
    range: TextRange,
    message: String,
    kind: SysmlDiagnosticKind,
) -> SysmlDiagnostic {
    SysmlDiagnostic {
        file: workspace.file_name(file).to_string(),
        kind,
        start: u32::from(range.start()),
        end: u32::from(range.end()),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_project_elements_with_standard_library() {
        let analysis = SysmlAnalysis::from_files([(
            "example.sysml",
            "package Example { part def Rover { } }",
        )]);
        assert_eq!(analysis.files().len(), 1);
        assert!(analysis
            .elements()
            .iter()
            .any(|element| element.qualified_name == "Example::Rover"));
        assert!(analysis.includes_stdlib());
    }

    #[test]
    fn malformed_source_is_reported_without_panicking() {
        let analysis = SysmlAnalysis::from_files_without_stdlib([("broken.sysml", "package {")]);
        assert!(analysis.has_errors());
        assert!(analysis
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.kind == SysmlDiagnosticKind::Syntax));
    }

    #[test]
    fn source_revision_is_preserved() {
        let analysis = SysmlAnalysis::build([("a.sysml", "part def A {}")], false, 42);
        assert_eq!(analysis.source_revision(), 42);
    }
}
