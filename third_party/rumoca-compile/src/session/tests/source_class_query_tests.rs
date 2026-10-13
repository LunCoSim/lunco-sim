use super::*;

#[test]
fn source_class_query_qualifies_exact_nested_and_quoted_document_paths() {
    let mut session = Session::default();
    session
        .add_document(
            "inline.mo",
            "within 'Source.Space'; package Box model 'Part.Dot' end 'Part.Dot'; end Box;",
        )
        .unwrap();
    for name in ["Box.'Part.Dot'", "'Source.Space'.Box.'Part.Dot'"] {
        assert_eq!(
            session.qualify_model_name("inline.mo", name),
            "'Source.Space'.Box.'Part.Dot'"
        );
    }
    assert_eq!(
        session.qualify_model_name("inline.mo", "Other.Model"),
        "Other.Model"
    );
}

#[test]
fn source_class_query_reuses_declaration_enumeration_for_session_completion() {
    let mut session = Session::default();
    session
        .add_document(
            "inline.mo",
            "within Source; package Box model Part end Part; end Box;",
        )
        .unwrap();
    let names = session.all_class_names_for_completion().unwrap();
    assert!(names.contains(&"Source.Box".to_owned()));
    assert!(names.contains(&"Source.Box.Part".to_owned()));
}

#[test]
fn namespace_map_revision_cache_preserves_edits_and_foreign_cache_validation() {
    let mut first = Session::default();
    first
        .add_document("first.mo", "model First end First;")
        .unwrap();
    assert!(first.class_definition_query("First").is_some());
    let mut other = Session::default();
    other
        .add_document("other.mo", "model Other end Other;")
        .unwrap();
    assert_eq!(first.revision(), other.revision());
    assert!(other.class_definition_query("Other").is_some());
    first
        .query_state
        .ast
        .package_def_map
        .merge_from(&other.query_state.ast.package_def_map);
    assert!(first.class_definition_query("First").is_some());
    assert!(first.class_definition_query("Other").is_none());
    assert!(
        first
            .update_document("first.mo", "model Updated end Updated;")
            .is_none()
    );
    assert!(first.class_definition_query("First").is_none());
    assert!(first.class_definition_query("Updated").is_some());
    first.remove_document("first.mo");
    assert!(first.class_definition_query("Updated").is_none());
}
