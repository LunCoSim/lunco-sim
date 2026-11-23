//! LunCo projections over Rumoca's public parsed-source queries.
//!
//! Rumoca owns parsing, class identity, lexical type candidates, and inherited
//! member selection. This module combines those public results with the source
//! AST facts LunCo's authoring and diagram consumers need.

use std::collections::HashSet;

use rumoca_compile::{Session, compile::WorkspaceSymbolKind};
use rumoca_ir_ast::{ClassDef, Component};

use crate::{Causality, ClassType, ModelicaDefinitionExt, PredefinedComponentType};

/// Parses exact source text through Rumoca and returns its AST and expanded
/// text together, preserving the source coordinates authored consumers read.
#[derive(Default)]
pub struct SourceFileParser {
    session: Session,
}

/// LunCo's serializable source facts paired with Rumoca's source-bearing
/// parsed document for compiler-session admission.
#[derive(Debug, Clone)]
pub struct ParsedSourceFile {
    pub snapshot: crate::resource_uri::SourceFileSnapshot,
    pub rumoca_document: rumoca_compile::compile::ParsedSourceDocument,
}

impl SourceFileParser {
    pub fn parse(
        &mut self,
        source_uri: impl Into<String>,
        source: impl Into<String>,
    ) -> Result<crate::resource_uri::SourceFileSnapshot, String> {
        self.parse_with_document(source_uri, source)
            .map(|parsed| parsed.snapshot)
    }

    /// Parse once through Rumoca and retain its source text alongside the
    /// serializable LunCo snapshot used by asset and authoring consumers.
    pub fn parse_with_document(
        &mut self,
        source_uri: impl Into<String>,
        source: impl Into<String>,
    ) -> Result<ParsedSourceFile, String> {
        let source_uri = source_uri.into();
        let source = source.into();
        let rumoca_document =
            rumoca_compile::compile::ParsedSourceDocument::parse(source_uri.clone(), source)
                .map_err(|error| format!("could not parse {source_uri}: {error}"))?;
        self.session
            .add_in_memory_parsed_batch(vec![rumoca_document.clone()]);
        let result = (|| {
            let definition = self
                .session
                .parsed_file_query(&source_uri)
                .cloned()
                .ok_or_else(|| format!("Rumoca did not retain parsed source `{source_uri}`"))?;
            let expanded = self
                .session
                .document_source_text(&source_uri)
                .ok_or_else(|| format!("Rumoca did not retain source text `{source_uri}`"))?;
            Ok(ParsedSourceFile {
                snapshot: crate::resource_uri::SourceFileSnapshot::new(
                    source_uri.clone(),
                    expanded.to_string(),
                    definition,
                ),
                rumoca_document: rumoca_document.clone(),
            })
        })();
        self.session.remove_document(&source_uri);
        result
    }
}

/// A component declaration projected for LunCo's authoring and diagram views.
#[derive(Debug, Clone)]
pub struct ResolvedComponentDeclaration {
    pub declaring_class: String,
    pub type_name: String,
    pub component: Component,
    pub effective_causality: Causality,
    pub scalar_type: Option<PredefinedComponentType>,
    pub has_type_dimensions: bool,
}

/// Type facts assembled from Rumoca's scoped type candidates and source AST.
#[derive(Debug, Clone)]
pub struct ResolvedTypePrefixes {
    pub qualified_type_name: String,
    pub causality: Causality,
    pub scalar_type: Option<PredefinedComponentType>,
    pub has_dimensions: bool,
}

/// Source declaration facts for one dotted component reference.
#[derive(Debug, Clone)]
pub struct ResolvedComponentReference {
    pub declaration: ResolvedComponentDeclaration,
    pub effective_variability: rumoca_core::Variability,
    pub is_indexed: bool,
    pub is_protected: bool,
    pub is_final: bool,
}

/// Retrieve a class from the exact documents admitted to a Rumoca session.
pub fn class_definition(session: &mut Session, class_name: &str) -> Option<ClassDef> {
    class_definition_ref(session, class_name).cloned()
}

fn class_definition_ref<'a>(session: &'a mut Session, class_name: &str) -> Option<&'a ClassDef> {
    let target = session.class_lookup_query(class_name)?;
    let leaf = crate::modelica_path_parts(&target).pop()?;
    for symbol in session.workspace_symbol_query(&leaf) {
        if symbol.name != leaf || !matches!(symbol.kind, WorkspaceSymbolKind::Class(_)) {
            continue;
        }
        if let Some(class) = session
            .get_document(&symbol.uri)
            .and_then(|document| document.parsed())
            .and_then(|definition| definition.class_by_path(&target))
        {
            return Some(class);
        }
    }
    None
}

/// Project Rumoca's inherited member selection into the source facts consumed
/// by LunCo. Unresolved types and cyclic aliases reject the complete projection.
pub fn component_declarations(
    session: &mut Session,
    class_name: &str,
) -> Option<Vec<ResolvedComponentDeclaration>> {
    let target = session.class_lookup_query(class_name)?;
    let members = session.class_component_members_query(&target);
    let mut declarations = Vec::with_capacity(members.len());
    for (name, _) in members {
        declarations.push(component_declaration(session, &target, &name)?);
    }
    Some(declarations)
}

fn component_declaration(
    session: &mut Session,
    class_name: &str,
    name: &str,
) -> Option<ResolvedComponentDeclaration> {
    let (declaring_class, _) = session.class_component_member_info_query(class_name, name)?;
    let class = class_definition_ref(session, &declaring_class)?;
    let component = class.components.get(name)?.clone();
    let prefixes =
        resolve_type_prefixes(session, &declaring_class, &component.type_name.to_string())?;
    let effective_causality = if matches!(component.causality, Causality::Empty) {
        prefixes.causality
    } else {
        component.causality.clone()
    };
    Some(ResolvedComponentDeclaration {
        declaring_class,
        type_name: prefixes.qualified_type_name,
        component,
        effective_causality,
        scalar_type: prefixes.scalar_type,
        has_type_dimensions: prefixes.has_dimensions,
    })
}

/// Resolve one authored component reference by composing Rumoca's class
/// identity, inherited-member and scoped-type queries.
pub fn component_reference(
    session: &mut Session,
    class_name: &str,
    reference: &str,
) -> Option<ResolvedComponentReference> {
    let mut current_class = session.class_lookup_query(class_name)?;
    let mut inherited_variability = None;
    let mut is_indexed = false;
    let mut is_protected = false;
    let mut is_final = false;
    let mut resolved = None;

    for segment in crate::modelica_path_parts(reference) {
        let component_name = match rumoca_core::parse_scalar_name(&segment) {
            Some(scalar) => {
                is_indexed = true;
                scalar.base.to_owned()
            }
            None => segment,
        };
        let declaration = component_declaration(session, &current_class, &component_name)?;
        let component = &declaration.component;
        let variability = if matches!(component.variability, rumoca_core::Variability::Empty) {
            inherited_variability
                .clone()
                .unwrap_or_else(|| component.variability.clone())
        } else {
            component.variability.clone()
        };
        is_protected |= component.is_protected;
        is_final |= component.is_final;
        inherited_variability = Some(variability);
        current_class = declaration.type_name.clone();
        resolved = Some(declaration);
    }

    Some(ResolvedComponentReference {
        declaration: resolved?,
        effective_variability: inherited_variability?,
        is_indexed,
        is_protected,
        is_final,
    })
}

/// Resolve a type spelling in its declaring class and follow primitive aliases.
pub fn resolve_type_prefixes(
    session: &mut Session,
    scope: &str,
    raw_name: &str,
) -> Option<ResolvedTypePrefixes> {
    resolve_type_prefixes_inner(session, scope, raw_name, &mut HashSet::new())
}

fn resolve_type_prefixes_inner(
    session: &mut Session,
    scope: &str,
    raw_name: &str,
    visiting: &mut HashSet<String>,
) -> Option<ResolvedTypePrefixes> {
    if rumoca_core::is_builtin_type(raw_name) {
        return Some(ResolvedTypePrefixes {
            qualified_type_name: raw_name.to_owned(),
            causality: Causality::Empty,
            scalar_type: PredefinedComponentType::from_name(raw_name),
            has_dimensions: false,
        });
    }

    let qualified = scoped_type_resolution_candidates(session, scope, raw_name)
        .into_iter()
        .find_map(|candidate| session.class_lookup_query(&candidate))?;
    if !visiting.insert(qualified.clone()) {
        return None;
    }

    let class = class_definition(session, &qualified)?;
    let primitive_alias = class.components.is_empty()
        && matches!(
            class.class_type,
            ClassType::Type | ClassType::Connector | ClassType::Class
        );
    let mut result = ResolvedTypePrefixes {
        qualified_type_name: qualified.clone(),
        causality: class.causality.clone(),
        scalar_type: None,
        has_dimensions: !class.array_subscripts.is_empty(),
    };

    for base in &class.extends {
        let inherited = resolve_type_prefixes_inner(
            session,
            &qualified,
            &base.base_name.to_string(),
            visiting,
        )?;
        result.has_dimensions |= inherited.has_dimensions;
        if primitive_alias {
            if result.scalar_type.is_some()
                && inherited.scalar_type.is_some()
                && result.scalar_type != inherited.scalar_type
            {
                return None;
            }
            result.scalar_type = result.scalar_type.or(inherited.scalar_type);
        }
        if matches!(result.causality, Causality::Empty) {
            result.causality = inherited.causality;
        } else if !matches!(inherited.causality, Causality::Empty)
            && std::mem::discriminant(&result.causality)
                != std::mem::discriminant(&inherited.causality)
        {
            return None;
        }
    }
    visiting.remove(&qualified);
    Some(result)
}

/// Complete Rumoca's type candidates with the package scopes that contain the
/// declaring class. Rumoca returns a bare candidate for sibling classes in a
/// `within Package;` source file; the session lookup resolves that bare name
/// only when it is globally unique. Trying enclosing packages first preserves
/// Modelica lexical scope when another package declares the same leaf name.
fn scoped_type_resolution_candidates(
    session: &mut Session,
    scope: &str,
    raw_name: &str,
) -> Vec<String> {
    let candidates = session.class_type_resolution_candidates_in_class_query(scope, raw_name);
    let mut scoped = Vec::with_capacity(candidates.len() + 2);
    let mut seen = HashSet::new();

    for candidate in candidates {
        if candidate == raw_name
            && !raw_name.starts_with('.')
            && rumoca_core::parent_scope(raw_name).is_none()
        {
            let mut parent = rumoca_core::parent_scope(scope);
            while let Some(parent_scope) = parent {
                let candidate = format!("{parent_scope}.{raw_name}");
                if seen.insert(candidate.clone()) {
                    scoped.push(candidate);
                }
                parent = rumoca_core::parent_scope(parent_scope);
            }
        }
        if seen.insert(candidate.clone()) {
            scoped.push(candidate);
        }
    }
    scoped
}

#[cfg(test)]
mod tests {
    use super::{component_declarations, resolve_type_prefixes};
    use rumoca_compile::Session;

    #[test]
    fn session_projection_resolves_inherited_members_and_type_aliases() {
        let mut session = Session::default();
        session
            .add_document(
                "fixture.mo",
                "package P type Flag = Boolean; connector InputFlag = input Flag; model Base InputFlag enabled; end Base; model Root extends Base; end Root; end P;",
            )
            .expect("fixture admission");

        let members = component_declarations(&mut session, "P.Root").expect("resolved members");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].declaring_class, "P.Base");
        assert_eq!(members[0].type_name, "P.InputFlag");
        assert!(matches!(
            members[0].effective_causality,
            rumoca_core::Causality::Input(_)
        ));
        assert_eq!(
            members[0].scalar_type,
            Some(rumoca_core::PredefinedComponentType::Boolean)
        );
    }

    #[test]
    fn type_projection_rejects_alias_cycles() {
        let mut session = Session::default();
        session
            .add_document(
                "cycle.mo",
                "package P type A = B; type B = A; model Root A value; end Root; end P;",
            )
            .expect("fixture admission");
        assert!(resolve_type_prefixes(&mut session, "P.Root", "A").is_none());
    }

    #[test]
    fn source_set_projection_resolves_sibling_types_in_their_within_package() {
        use rumoca_compile::compile::{ParsedSourceDocument, SourceRootKind};

        let mut session = Session::default();
        let documents = [
            (
                "file:///twin/models/FactoryEquipment/package.mo",
                "within; package FactoryEquipment end FactoryEquipment;",
            ),
            (
                "file:///twin/models/FactoryEquipment/Identified.mo",
                "within FactoryEquipment; partial model Identified parameter String assetId=\"\"; end Identified;",
            ),
            (
                "file:///twin/models/FactoryEquipment/LotOut.mo",
                "within FactoryEquipment; connector LotOut Real lotKg; end LotOut;",
            ),
            (
                "file:///twin/models/FactoryEquipment/Feed.mo",
                "within FactoryEquipment; model Feed extends Identified; LotOut outlet; end Feed;",
            ),
            (
                "file:///twin/models/A/package.mo",
                "within; package A end A;",
            ),
            (
                "file:///twin/models/A/Shared.mo",
                "within A; partial model Shared parameter Real aValue=1; end Shared;",
            ),
            (
                "file:///twin/models/A/Feed.mo",
                "within A; model Feed extends Shared; end Feed;",
            ),
            (
                "file:///twin/models/B/package.mo",
                "within; package B end B;",
            ),
            (
                "file:///twin/models/B/Shared.mo",
                "within B; partial model Shared parameter Real bValue=2; end Shared;",
            ),
        ]
        .into_iter()
        .map(|(uri, source)| ParsedSourceDocument::parse(uri, source).expect("fixture parses"))
        .collect();
        session.replace_in_memory_source_set(
            "twin:1:models",
            SourceRootKind::DurableExternal,
            documents,
            None,
        );

        let feed = component_declarations(&mut session, "FactoryEquipment.Feed")
            .expect("same-package declarations resolve");
        assert!(
            feed.iter().any(|member| {
                member.component.name == "assetId" && member.type_name == "String"
            })
        );
        assert!(feed.iter().any(|member| {
            member.component.name == "outlet" && member.type_name == "FactoryEquipment.LotOut"
        }));

        let scoped = component_declarations(&mut session, "A.Feed")
            .expect("lexically scoped duplicate type resolves");
        assert!(
            scoped
                .iter()
                .any(|member| member.component.name == "aValue")
        );
        assert!(
            !scoped
                .iter()
                .any(|member| member.component.name == "bValue")
        );
    }
}
