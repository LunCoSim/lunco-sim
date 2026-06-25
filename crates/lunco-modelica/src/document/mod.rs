//! Document management for Modelica files.

pub mod ops;
pub mod core;
pub mod apply;
pub mod duplicate;

pub use core::{ModelicaDocument, SyntaxCache, AstCache, ParseDiag, parse_diag_from_error};
pub use ops::{ModelicaOp, ModelicaChange, OpKind, FreshAst, CHANGE_HISTORY_CAPACITY};

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::{DocumentHost, DocumentId, Reject};
    use crate::pretty::ComponentDecl;

    fn doc() -> DocumentHost<ModelicaDocument> {
        DocumentHost::new(ModelicaDocument::new(
            DocumentId::new(1),
            "model Empty end Empty;\n",
        ))
    }

    #[test]
    fn fresh_document_state() {
        let host = doc();
        // A fresh document starts at generation 1: the constructor seeds an
        // empty placeholder SyntaxCache at gen 0 and bumps the doc to gen 1 so
        // the AST reads as stale and the lazy parse / index rebuild fires.
        assert_eq!(host.generation(), 1);
        assert_eq!(host.document().source(), "model Empty end Empty;\n");
        assert_eq!(host.document().id_owned(), DocumentId::new(1));
        assert!(!host.can_undo());
        assert!(!host.can_redo());
        assert!(!host.document().is_empty());
    }

    #[test]
    fn replace_source_mutates_and_bumps_generation() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource {
            new: "model NewModel end NewModel;".into(),
        })
        .unwrap();
        assert_eq!(host.document().source(), "model NewModel end NewModel;");
        assert_eq!(host.generation(), 2); // gen 1 fresh + 1 mutation
        assert!(host.can_undo());
    }

    #[test]
    fn undo_restores_previous_source() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource {
            new: "replaced".into(),
        })
        .unwrap();
        host.undo().unwrap();
        assert_eq!(host.document().source(), "model Empty end Empty;\n");
    }

    #[test]
    fn redo_reapplies_replaced_source() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource {
            new: "replaced".into(),
        })
        .unwrap();
        host.undo().unwrap();
        host.redo().unwrap();
        assert_eq!(host.document().source(), "replaced");
    }

    #[test]
    fn multi_step_undo_redo_round_trip() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource { new: "a".into() }).unwrap();
        host.apply(ModelicaOp::ReplaceSource { new: "b".into() }).unwrap();
        host.apply(ModelicaOp::ReplaceSource { new: "c".into() }).unwrap();
        assert_eq!(host.document().source(), "c");
        assert_eq!(host.generation(), 4); // gen 1 fresh + 3 mutations

        host.undo().unwrap();
        host.undo().unwrap();
        host.undo().unwrap();
        assert_eq!(host.document().source(), "model Empty end Empty;\n");

        host.redo().unwrap();
        host.redo().unwrap();
        host.redo().unwrap();
        assert_eq!(host.document().source(), "c");
    }

    #[test]
    fn generation_monotonic_across_undo_redo() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource { new: "a".into() }).unwrap();
        assert_eq!(host.generation(), 2); // gen 1 fresh + 1 mutation
        host.undo().unwrap();
        assert_eq!(host.generation(), 3);
        host.redo().unwrap();
        assert_eq!(host.generation(), 4);
    }

    #[test]
    fn new_apply_clears_redo_branch() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource { new: "first".into() }).unwrap();
        host.undo().unwrap();
        assert!(host.can_redo());

        host.apply(ModelicaOp::ReplaceSource { new: "second".into() }).unwrap();
        assert!(!host.can_redo());
        assert_eq!(host.document().source(), "second");
    }

    #[test]
    fn ast_cache_refreshes_after_mutation() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource {
            new: "model Foo end Foo;".into(),
        })
        .unwrap();
        host.document_mut().refresh_ast_now();
        let cache = host.document().ast();
        assert_eq!(cache.generation, 2); // gen 1 fresh + 1 mutation, then refreshed
        let ast = host.document().strict_ast().expect("strict_ast Some");
        assert!(ast.classes.contains_key("Foo"));
        assert!(!ast.classes.contains_key("Empty"));
    }

    #[test]
    fn ast_stays_stale_until_refresh() {
        let mut host = doc();
        host.apply(ModelicaOp::ReplaceSource {
            new: "model Foo end Foo;".into(),
        })
        .unwrap();
        assert!(
            host.document().ast_is_stale(),
            "AST should be stale right after apply_patch"
        );
        assert_eq!(host.document().ast().generation, 0); // empty placeholder, never refreshed
        host.document_mut().refresh_ast_now();
        assert!(!host.document().ast_is_stale());
        assert_eq!(host.document().ast().generation, 2); // gen 1 fresh + 1 mutation, then refreshed
    }

    #[test]
    fn edit_text_replaces_range_and_is_invertible() {
        let mut host = doc();
        host.apply(ModelicaOp::EditText {
            range: 6..11,
            replacement: "Thing".into(),
        })
        .unwrap();
        assert_eq!(host.document().source(), "model Thing end Empty;\n");
        assert_eq!(host.generation(), 2); // gen 1 fresh + 1 mutation

        host.undo().unwrap();
        assert_eq!(host.document().source(), "model Empty end Empty;\n");
    }

    #[test]
    fn edit_text_supports_insertion_and_deletion() {
        let mut host = DocumentHost::new(ModelicaDocument::new(
            DocumentId::new(1),
            "abcdef".to_string(),
        ));
        host.apply(ModelicaOp::EditText {
            range: 3..3,
            replacement: "XYZ".into(),
        })
        .unwrap();
        assert_eq!(host.document().source(), "abcXYZdef");

        host.apply(ModelicaOp::EditText {
            range: 3..6,
            replacement: String::new(),
        })
        .unwrap();
        assert_eq!(host.document().source(), "abcdef");

        host.undo().unwrap();
        assert_eq!(host.document().source(), "abcXYZdef");
        host.undo().unwrap();
        assert_eq!(host.document().source(), "abcdef");
    }

    #[test]
    fn edit_text_rejects_out_of_bounds_range() {
        let mut host = doc();
        let err = host
            .apply(ModelicaOp::EditText {
                range: 0..999,
                replacement: String::new(),
            })
            .unwrap_err();
        assert!(matches!(err, Reject::InvalidOp(_)));
        assert_eq!(host.document().source(), "model Empty end Empty;\n");
        assert_eq!(host.generation(), 1); // rejected op leaves the fresh gen 1 untouched
    }

    #[test]
    fn add_component_appends_before_end_when_no_equation_section() {
        let mut host = DocumentHost::new(ModelicaDocument::new(
            DocumentId::new(1),
            "model M\n  Real a;\nend M;\n".to_string(),
        ));
        // Parsing is lazy: refresh so the AST holds class M before the
        // structural mutation (the live app parses async after construction).
        host.document_mut().refresh_ast_now();
        host.apply(ModelicaOp::AddComponent {
            class: "M".into(),
            decl: ComponentDecl {
                type_name: "Real".into(),
                name: "b".into(),
                modifications: vec![],
                placement: None,
            },
        })
        .unwrap();
        assert_eq!(
            host.document().source(),
            "model M\n  Real a;\n  Real b;\nend M;\n"
        );
        host.document_mut().refresh_ast_now();
        let ast = host.document().strict_ast().expect("parse ok");
        assert!(ast.classes.get("M").unwrap().components.contains_key("b"));
    }

    #[test]
    fn add_component_is_invertible() {
        let original = "model M\n  Real a;\nend M;\n";
        let mut host = DocumentHost::new(ModelicaDocument::new(
            DocumentId::new(1),
            original.to_string(),
        ));
        // Parsing is lazy: refresh so the AST holds class M before the mutation.
        host.document_mut().refresh_ast_now();
        host.apply(ModelicaOp::AddComponent {
            class: "M".into(),
            decl: ComponentDecl {
                type_name: "Real".into(),
                name: "b".into(),
                modifications: vec![],
                placement: None,
            },
        })
        .unwrap();
        host.undo().unwrap();
        assert_eq!(host.document().source(), original);
    }
}
