//! `UndoDocument` / `RedoDocument` — the ONE undo, tested at the document level.
//!
//! These exercise `DocumentHost::undo()/redo()` directly: the command observers
//! (`on_undo_usd_document` / `on_redo_usd_document` in `src/commands.rs`) are a thin
//! per-domain dispatch over exactly this, so the invariants live here.
//!
//! Every authored edit reaches the world as a `UsdOp`, and `UsdDocument::apply` hands
//! back a typed inverse. So undo is a property of the document and covers every verb
//! automatically — including ones nobody wrote undo code for. That is the property
//! the editor's old `Vec<UndoAction>` stack could not have: it knew exactly two verbs
//! and never touched the document.

use lunco_doc::{Document, DocumentHost, DocumentId, Mutation};
use lunco_usd::document::{LayerId, UsdDocument, UsdOp};

const STAGE: &str = r#"#usda 1.0
(
    defaultPrim = "World"
)

def Xform "World"
{
}
"#;

fn host() -> DocumentHost<UsdDocument> {
    DocumentHost::new(UsdDocument::new(DocumentId::new(1), STAGE))
}

fn has_prim(host: &DocumentHost<UsdDocument>, path: &str) -> bool {
    let sdf = openusd::sdf::Path::new(path).expect("valid path");
    host.document().data().spec(&sdf).is_some()
        || host.document().runtime_data().spec(&sdf).is_some()
}

fn add_prim(host: &mut DocumentHost<UsdDocument>, name: &str) {
    host.apply(Mutation::local(UsdOp::AddPrim {
        edit_target: LayerId::runtime(),
        parent_path: "/World".into(),
        name: name.into(),
        type_name: Some("Xform".into()),
        reference: None,
    }))
    .expect("AddPrim applies");
}

#[test]
fn undo_removes_an_authored_prim_and_redo_restores_it() {
    // The spawn case the editor stack got WRONG: it despawned the entity but left the
    // prim in the layer, so the document and the scene disagreed and the next
    // projection could bring it back. The document's inverse removes the prim itself.
    let mut host = host();
    add_prim(&mut host, "Rover");
    assert!(has_prim(&host, "/World/Rover"));

    assert!(host.undo().expect("undo runs"), "there was an op to undo");
    assert!(
        !has_prim(&host, "/World/Rover"),
        "undo must remove the authored prim, not just its entity"
    );

    assert!(host.redo().expect("redo runs"), "there was an op to redo");
    assert!(has_prim(&host, "/World/Rover"), "redo restores it");
}

#[test]
fn undo_covers_verbs_nobody_wrote_undo_code_for() {
    // A waypoint drop authors AddPrim + SetTranslate + SetAttribute. None of those had
    // a hand-written undo action, and none needed one: each carries its own typed
    // inverse. Undoing three times peels the whole edit off.
    let mut host = host();
    add_prim(&mut host, "wp1");
    host.apply(Mutation::local(UsdOp::SetTranslate {
        edit_target: LayerId::runtime(),
        path: "/World/wp1".into(),
        value: [10.0, 0.0, 3.0],
    }))
    .expect("SetTranslate applies");
    host.apply(Mutation::local(UsdOp::SetAttribute {
        edit_target: LayerId::runtime(),
        path: "/World".into(),
        // Arbitrary custom attribute — this test is about the typed inverse of
        // SetAttribute, not about any particular attribute's meaning.
        name: "lunco:test:note".into(),
        type_name: "string".into(),
        value: "<root/>".into(),
    }))
    .expect("SetAttribute applies");

    assert_eq!(host.undo_depth(), 3, "three authored ops, three inverses");
    for _ in 0..3 {
        assert!(host.undo().expect("undo runs"));
    }
    assert!(
        !has_prim(&host, "/World/wp1"),
        "undoing the whole waypoint edit removes the pin prim"
    );
    assert_eq!(host.undo_depth(), 0);
}

#[test]
fn undo_on_a_clean_document_is_a_no_op_not_an_error() {
    let mut host = host();
    assert!(!host.can_undo());
    assert!(
        !host.undo().expect("undo on an empty stack must not error"),
        "nothing to undo → Ok(false)"
    );
}
