use super::*;

fn assert_typecheck_failure(result: PhaseResult, expected_code: &str) {
    match result {
        PhaseResult::Failed {
            phase, error_code, ..
        } => {
            assert_eq!(phase, FailedPhase::Typecheck);
            assert_eq!(error_code.as_deref(), Some(expected_code));
        }
        other => panic!("expected typecheck failure, got {other:?}"),
    }
}

#[test]
fn typed_model_query_cache_is_reused_by_compile_and_diagnostics() {
    let mut session = Session::default();
    session
        .add_document(
            "target.mo",
            "model Target\n  Real x(startd = 1.0);\nequation\n  der(x) = -x;\nend Target;\n",
        )
        .expect("target should parse");
    session
        .add_document("other.mo", "model Other\n  Real y;\nend Other;\n")
        .expect("other should parse");

    let first = session
        .compile_model_phases("Target")
        .expect("compile should return phase result");
    assert_typecheck_failure(first, "ET001");

    let cache_key = standard_typed_cache_key(&mut session, "Target");
    match session
        .query_state
        .flat
        .typed_models
        .artifacts
        .get(&cache_key)
        .expect("typed query should be cached")
        .outcome
        .clone()
    {
        TypedModelOutcome::TypecheckError(diags) => {
            assert!(
                diags
                    .iter()
                    .any(|diag| diag.code.as_deref() == Some("ET001")),
                "initial typed artifact should preserve the real typecheck failure"
            );
        }
        other => panic!("expected cached typecheck error, got {other:?}"),
    }

    session
        .query_state
        .flat
        .typed_models
        .artifacts
        .get_mut(&cache_key)
        .expect("typed query should be cached")
        .outcome = TypedModelOutcome::TypecheckError(vec![CommonDiagnostic::error(
        "TTEST",
        "typed cache sentinel",
        PrimaryLabel::new(Span::DUMMY).with_message("cache sentinel"),
    )]);

    let parse_error = session.update_document(
        "other.mo",
        "model Other\n  Real y;\n  Real z;\nend Other;\n",
    );
    assert!(parse_error.is_none(), "unrelated edit should remain valid");

    session
        .query_state
        .dae
        .compile_results
        .shift_remove("Target");
    let dae_cache_key = standard_dae_cache_key(&mut session, "Target");
    session
        .query_state
        .dae
        .dae_models
        .shift_remove(&dae_cache_key);
    let flat_cache_key = standard_flat_cache_key(&mut session, "Target");
    session
        .query_state
        .flat
        .flat_models
        .artifacts
        .shift_remove(&flat_cache_key);

    let warm_compile = session
        .compile_model_phases("Target")
        .expect("warm compile should return phase result");
    assert_typecheck_failure(warm_compile, "TTEST");

    let warm_diagnostics = session.compile_model_diagnostics("Target");
    assert_diagnostics_have_code(&warm_diagnostics, "TTEST");

    let parse_error = session.update_document(
        "target.mo",
        "model Target\n  Real x(start = 1.0);\nequation\n  der(x) = -x;\nend Target;\n",
    );
    assert!(parse_error.is_none(), "target edit should remain valid");

    let rebuilt = session
        .compile_model_phases("Target")
        .expect("rebuild should return phase result");
    assert!(
        matches!(rebuilt, PhaseResult::Success(_)),
        "target edit should rebuild the typed model instead of reusing the stale typecheck failure"
    );

    let rebuilt_diagnostics = session.compile_model_diagnostics("Target");
    assert_diagnostics_lack_code(&rebuilt_diagnostics, "TTEST");
}

#[test]
fn class_definition_query_preserves_exact_within_and_quoted_identity() {
    let mut session = Session::default();
    session
        .add_document(
            "within.mo",
            "within Space; package Root model 'A.B' Real marker; end 'A.B'; end Root;",
        )
        .expect("valid source");
    let selected = session
        .class_definition_query("Space.Root.'A.B'")
        .expect("exact qualified declaration");
    assert_eq!(selected.name.text.as_ref(), "'A.B'");
    assert!(selected.components.contains_key("marker"));
    let selected_ptr = selected as *const ast::ClassDef;
    let parsed = session
        .parsed_file_query("within.mo")
        .expect("parsed document");
    assert_eq!(
        selected_ptr,
        parsed
            .get_class_by_qualified_name("Space.Root.'A.B'")
            .expect("same declaration") as *const ast::ClassDef
    );
    assert!(parsed.get_class_by_qualified_name("Root.'A.B'").is_some());
    for invalid in [
        "Other.Space.Root.'A.B'",
        "Space.Root..'A.B'",
        "Space.Root.'A.B'.",
        "Space.Root.A.B",
    ] {
        assert!(
            session.class_definition_query(invalid).is_none(),
            "{invalid}"
        );
    }
}

#[test]
fn class_definition_query_rejects_ambiguous_bare_targets_and_tracks_revision() {
    let mut session = Session::default();
    session.add_document("roots.mo", "package A model Shared Real first; end Shared; end A; package B model Shared Real second; end Shared; end B;").expect("valid source");
    assert!(session.class_definition_query("Shared").is_none());
    assert!(
        session
            .class_definition_query("A.Shared")
            .expect("A")
            .components
            .contains_key("first")
    );
    assert!(
        session
            .class_definition_query("B.Shared")
            .expect("B")
            .components
            .contains_key("second")
    );
    assert!(
        session
            .update_document(
                "roots.mo",
                "package B model Shared Real edited; end Shared; end B;"
            )
            .is_none()
    );
    assert!(session.class_definition_query("A.Shared").is_none());
    assert!(
        session
            .class_definition_query("Shared")
            .expect("unique current declaration")
            .components
            .contains_key("edited")
    );
    session.remove_document("roots.mo");
    assert!(session.class_definition_query("B.Shared").is_none());
}

#[test]
fn class_definition_query_tracks_preparsed_source_set_replacement() {
    let mut session = Session::default();
    session.replace_in_memory_source_set(
        "library",
        SourceRootKind::DurableExternal,
        vec![parse_source_document(
            "within Library; package P model Leaf Real original; end Leaf; end P;",
            "library.mo",
        )],
        None,
    );
    assert!(
        session
            .class_definition_query("Library.P.Leaf")
            .expect("installed parsed source")
            .components
            .contains_key("original")
    );
    session.replace_in_memory_source_set(
        "library",
        SourceRootKind::DurableExternal,
        vec![parse_source_document(
            "within Library; package P model Leaf Real replacement; end Leaf; end P;",
            "library.mo",
        )],
        None,
    );
    let current = session
        .class_definition_query("Library.P.Leaf")
        .expect("replacement declaration");
    assert!(current.components.contains_key("replacement"));
    assert!(!current.components.contains_key("original"));
}

#[test]
fn class_definition_query_component_declarations_are_typed_and_complete() {
    let mut session = Session::default();
    session.add_document("members.mo", "within Space; package P model Base parameter Real k=2; input Real u; end Base; model Derived extends Base; output Real y; end Derived; model Broken extends Missing; Real local; end Broken; model Cyclic extends Cyclic; end Cyclic; model Empty end Empty; type Scalar = Real; connector ScalarInput = input Scalar; end P;").expect("valid source");
    let members = session
        .class_component_declarations_query("Space.P.Derived")
        .expect("resolved declarations");
    assert_eq!(members.len(), 3);
    let parameter = members
        .iter()
        .find(|member| member.component.name == "k")
        .expect("inherited parameter");
    assert_eq!(parameter.declaring_class, "Space.P.Base");
    assert!(matches!(
        parameter.component.variability,
        rumoca_core::Variability::Parameter(_)
    ));
    assert_eq!(
        parameter
            .component
            .binding
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("2")
    );
    let input = members
        .iter()
        .find(|member| member.component.name == "u")
        .expect("inherited input");
    assert!(matches!(
        input.effective_causality,
        rumoca_core::Causality::Input(_)
    ));
    for invalid in ["Space.P.Broken", "Space.P.Cyclic", "Space.P.Unknown"] {
        assert!(
            session
                .class_component_declarations_query(invalid)
                .is_none(),
            "{invalid}"
        );
    }
    assert!(
        session
            .class_component_declarations_query("Space.P.Empty")
            .expect("valid empty class")
            .is_empty()
    );
    for primitive in ["Space.P.Scalar", "Space.P.ScalarInput"] {
        assert!(
            session
                .class_component_declarations_query(primitive)
                .expect("primitive-derived declaration has no named members")
                .is_empty()
        );
    }
    let mut parsed = session
        .parsed_file_query("members.mo")
        .expect("parsed file")
        .clone();
    parsed
        .get_class_by_qualified_name_mut("Space.P.Base")
        .expect("exact mutable declaration")
        .components
        .clear();
    assert!(
        parsed
            .get_class_by_qualified_name_mut("Other.Space.P.Base")
            .is_none()
    );
}

#[test]
fn class_component_declarations_resolve_type_prefixes_and_alias_dimensions() {
    let mut session = Session::default();
    session.add_document("prefix.mo", "package Facts connector Receiver=input Real; connector Sender=output Real; connector Again=Receiver; type Vector=Real[2]; type Caption=String; record FauxInput Real pressure; end FauxInput; model Probe Receiver u; Again u2; Sender y; FauxInput internal; parameter Vector values={1,2}; parameter Caption label=\"label\"; end Probe; connector CycleA=CycleB; connector CycleB=CycleA; model Cyclic CycleA x; end Cyclic; model Missing Absent x; end Missing; end Facts;").expect("valid source");
    let members = session
        .class_component_declarations_query("Facts.Probe")
        .expect("resolved types");
    for name in ["u", "u2"] {
        let member = members
            .iter()
            .find(|member| member.component.name == name)
            .unwrap();
        assert!(matches!(
            member.effective_causality,
            rumoca_core::Causality::Input(_)
        ));
        assert_eq!(
            member.scalar_type,
            Some(rumoca_core::PredefinedComponentType::Real)
        );
        assert!(matches!(
            member.component.causality,
            rumoca_core::Causality::Empty
        ));
    }
    let output = members
        .iter()
        .find(|member| member.component.name == "y")
        .unwrap();
    assert!(matches!(
        output.effective_causality,
        rumoca_core::Causality::Output(_)
    ));
    let internal = members
        .iter()
        .find(|member| member.component.name == "internal")
        .unwrap();
    assert!(matches!(
        internal.effective_causality,
        rumoca_core::Causality::Empty
    ));
    assert_eq!(internal.scalar_type, None);
    let vector = members
        .iter()
        .find(|member| member.component.name == "values")
        .unwrap();
    assert!(vector.has_type_dimensions);
    assert!(vector.component.shape_expr.is_empty());
    let caption = members
        .iter()
        .find(|member| member.component.name == "label")
        .unwrap();
    assert_eq!(
        caption.scalar_type,
        Some(rumoca_core::PredefinedComponentType::String)
    );
    for invalid in ["Facts.Missing", "Facts.Cyclic"] {
        assert!(
            session
                .class_component_declarations_query(invalid)
                .is_none()
        );
    }
    session
        .add_document(
            "elsewhere.mo",
            "package Elsewhere model Absent end Absent; end Elsewhere;",
        )
        .unwrap();
    assert!(
        session
            .class_component_declarations_query("Facts.Missing")
            .is_none(),
        "a globally unique suffix does not satisfy a lexical type reference"
    );
}

#[test]
fn class_component_declarations_respect_parent_imports_and_encapsulation() {
    let mut session = Session::default();
    session
        .add_document(
            "external.mo",
            "package External connector Signal=input Real; end External;",
        )
        .unwrap();
    session.add_document("parent.mo", "package Root import Alias=External.Signal; model Child Alias u; end Child; encapsulated model Isolated Alias u; end Isolated; encapsulated model Explicit import Alias=External.Signal; Alias u; end Explicit; end Root;").unwrap();
    for target in ["Root.Child", "Root.Explicit"] {
        let declarations = session
            .class_component_declarations_query(target)
            .expect("visible import");
        assert!(matches!(
            declarations[0].effective_causality,
            rumoca_core::Causality::Input(_)
        ));
        let hit = session.class_component_declarations_query(target).unwrap();
        assert!(std::sync::Arc::ptr_eq(&declarations, &hit));
    }
    assert!(
        session
            .class_component_declarations_query("Root.Isolated")
            .is_none()
    );
    session
        .add_document(
            "external.mo",
            "package External connector Signal=output Real; end External;",
        )
        .unwrap();
    let changed = session
        .class_component_declarations_query("Root.Child")
        .unwrap();
    assert!(matches!(
        changed[0].effective_causality,
        rumoca_core::Causality::Output(_)
    ));
}

#[test]
fn class_component_declarations_resolve_inherited_types_and_package_aliases() {
    let mut session = Session::default();
    session.add_document("inherited-types.mo", "package Types connector Signal=input Real; end Types; package Alias=Types; model Base connector Signal=output Real; end Base; model Derived extends Base; Signal y; end Derived; model ThroughPackage Alias.Signal u; end ThroughPackage;").unwrap();
    let inherited = session
        .class_component_declarations_query("Derived")
        .unwrap();
    assert!(matches!(
        inherited[0].effective_causality,
        rumoca_core::Causality::Output(_)
    ));
    let package = session
        .class_component_declarations_query("ThroughPackage")
        .unwrap();
    assert!(matches!(
        package[0].effective_causality,
        rumoca_core::Causality::Input(_)
    ));
}

#[test]
fn class_component_declarations_do_not_replace_a_bound_qualified_head() {
    let mut session = Session::default();
    session.add_document("shadow.mo", "package P connector Signal=input Real; end P; package Local package P connector Different=output Real; end P; model Missing P.Signal u; end Missing; end Local;").unwrap();
    assert!(
        session
            .class_component_declarations_query("Local.Missing")
            .is_none()
    );
}

#[test]
fn class_component_declarations_do_not_replace_an_explicit_import_head() {
    let mut session = Session::default();
    session.add_document("import-shadow.mo", "package P connector Signal=input Real; end P; package Source connector Different=output Real; end Source; model Missing import P=Source; P.Signal u; end Missing;").unwrap();
    assert!(
        session
            .class_component_declarations_query("Missing")
            .is_none()
    );
}

#[test]
fn declaration_types_preserve_exact_scoped_identity() {
    let mut session = Session::default();
    session
        .add_document(
            "scope.mo",
            r#"
package A
  connector Port Real potential; flow Real transfer; end Port;
  model Base Port inlet; end Base;
  model Derived extends Base; end Derived;
end A;
package B
  connector Port = input Real;
end B;
model Root
  import Selected = A.Derived;
  Selected child;
end Root;
"#,
        )
        .unwrap();
    let root = session.class_component_declarations_query("Root").unwrap();
    assert_eq!(root[0].type_name, "A.Derived");
    assert_eq!(root[0].component.type_name.to_string(), "Selected");
    let derived = session
        .class_component_declarations_query("A.Derived")
        .unwrap();
    assert_eq!(derived[0].type_name, "A.Port");
    assert_eq!(derived[0].declaring_class, "A.Base");
    let flows = session
        .class_component_declarations_query("A.Port")
        .unwrap();
    assert!(flows.iter().all(|member| member.type_name == "Real"));
}
