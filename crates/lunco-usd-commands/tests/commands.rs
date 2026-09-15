//! Integration coverage for the public USD command and document lifecycle boundary.
//!
//! These tests exercise the production plugin through its public command and
//! event contracts. Private pending-load and grouped-edit seams remain unit
//! tests beside their implementation.

use bevy::prelude::*;
use lunco_core::{ActiveCommandId, CommandResults};
use lunco_doc::DocumentId;
use lunco_doc_bevy::{
    DiscardDocument, DocumentRegistry, ForkDocument, NewDocument, OpenFile, UndoDocument,
};
use lunco_twin::{DocumentKindId, DocumentKindRegistry};
use lunco_usd_commands::UsdCommandsPlugin;
use lunco_usd_core::commands::{
    ApplyUsdOp, CommitUsdProposal, CreateUsdProposal, ReviewUsdProposal, UsdProposalReviewAction,
    USD_DOCUMENT_KIND,
};
use lunco_usd_core::edit_session::{UsdEditScope, UsdEditSessions, UsdProposalState};
use lunco_usd_document::document::{LayerId, UsdDocument, UsdOp};

fn install_command_result_resources(app: &mut App) {
    app.init_resource::<CommandResults>()
        .init_resource::<ActiveCommandId>();
}

#[test]
fn plugin_boots_and_registers_kind() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(UsdCommandsPlugin);
    app.update();

    assert!(app
        .world()
        .contains_resource::<DocumentRegistry<UsdDocument>>());
    let kinds = app.world().resource::<DocumentKindRegistry>();
    let meta = kinds
        .meta(&DocumentKindId::new(USD_DOCUMENT_KIND))
        .expect("usd kind registered");
    assert_eq!(meta.display_name, "USD Stage");
    assert_eq!(meta.extensions, vec!["usda", "usdc", "usd"]);
    assert!(app
        .world()
        .resource::<lunco_api::queries::ApiQueryRegistry>()
        .names()
        .any(|name| name == "InspectUsdDocument"));
    assert!(app
        .world()
        .resource::<lunco_api::queries::ApiQueryRegistry>()
        .names()
        .any(|name| name == "InspectUsdEditSession"));
    assert!(app
        .world()
        .resource::<lunco_api::queries::ApiQueryRegistry>()
        .names()
        .any(|name| name == "ResolveUsdTarget"));
    assert!(app
        .world()
        .resource::<lunco_api::queries::ApiQueryRegistry>()
        .names()
        .any(|name| name == "SyncUsdDocument"));
}

fn proposal_test_op(name: &str) -> UsdOp {
    UsdOp::AddPrim {
        edit_target: LayerId::root(),
        parent_path: "/Assembly".to_owned(),
        name: name.to_owned(),
        type_name: Some("Xform".to_owned()),
        reference: None,
        reference_prim_path: None,
    }
}

fn proposal_test_app() -> (App, DocumentId) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(UsdCommandsPlugin);
    install_command_result_resources(&mut app);
    app.update();
    let doc = app
        .world_mut()
        .resource_mut::<DocumentRegistry<UsdDocument>>()
        .allocate(
            "#usda 1.0\ndef Xform \"Assembly\" {}\n".to_owned(),
            lunco_doc::PathlessOrigin::untitled("Proposal.usda"),
        );
    app.update();
    (app, doc)
}

#[test]
fn proposal_commands_keep_review_separate_from_usd_and_commit_as_one_edit() {
    let (mut app, doc) = proposal_test_app();
    let op = proposal_test_op("Chassis");
    app.world_mut().trigger(CreateUsdProposal {
        doc_id: doc,
        scope: UsdEditScope::Assembly,
        label: "Add chassis".to_owned(),
        parent_gen: 0,
        ops: vec![op],
    });
    app.update();

    let proposal = {
        let sessions = app.world().resource::<UsdEditSessions>();
        assert_eq!(sessions.for_document(doc).count(), 1);
        let proposal = sessions.for_document(doc).next().expect("proposal");
        assert_eq!(proposal.parent_generation, 0);
        assert_eq!(proposal.ops.len(), 1);
        proposal.id
    };
    assert!(!app
        .world()
        .resource::<DocumentRegistry<UsdDocument>>()
        .host(doc)
        .expect("document")
        .document()
        .source()
        .contains("Chassis"));

    app.world_mut().trigger(ReviewUsdProposal {
        proposal,
        action: UsdProposalReviewAction::Mute,
    });
    app.update();
    assert_eq!(
        app.world()
            .resource::<UsdEditSessions>()
            .for_document(doc)
            .filter(|proposal| proposal.state == UsdProposalState::Muted)
            .count(),
        1
    );
    app.world_mut().trigger(ReviewUsdProposal {
        proposal,
        action: UsdProposalReviewAction::Unmute,
    });
    app.update();
    assert_eq!(
        app.world()
            .resource::<UsdEditSessions>()
            .for_document(doc)
            .filter(|proposal| proposal.state == UsdProposalState::Pending)
            .count(),
        1
    );

    app.world_mut().trigger(CommitUsdProposal { proposal });
    app.update();
    let registry = app.world().resource::<DocumentRegistry<UsdDocument>>();
    let host = registry.host(doc).expect("committed document");
    assert_eq!(host.generation(), 1);
    assert!(host.document().source().contains("Chassis"));
    assert!(app
        .world()
        .resource::<UsdEditSessions>()
        .proposal(proposal)
        .is_none());

    app.world_mut().trigger(UndoDocument { doc_id: doc });
    app.update();
    assert!(!app
        .world()
        .resource::<DocumentRegistry<UsdDocument>>()
        .host(doc)
        .expect("undo document")
        .document()
        .source()
        .contains("Chassis"));
}

#[test]
fn proposal_commit_marks_a_stale_plan_as_conflict_without_overwriting_edits() {
    let (mut app, doc) = proposal_test_app();
    app.world_mut().trigger(CreateUsdProposal {
        doc_id: doc,
        scope: UsdEditScope::Assembly,
        label: "Add stale chassis".to_owned(),
        parent_gen: 0,
        ops: vec![proposal_test_op("Chassis")],
    });
    app.update();
    let proposal = app
        .world()
        .resource::<UsdEditSessions>()
        .for_document(doc)
        .next()
        .expect("proposal")
        .id;

    app.world_mut()
        .resource_mut::<DocumentRegistry<UsdDocument>>()
        .apply(doc, proposal_test_op("ExistingEdit"))
        .expect("independent edit");
    app.world_mut().trigger(CommitUsdProposal { proposal });
    app.update();

    let sessions = app.world().resource::<UsdEditSessions>();
    let conflicted = sessions.proposal(proposal).expect("conflict retained");
    assert_eq!(conflicted.state, UsdProposalState::Conflict);
    assert!(conflicted
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains("stale document generation")));
    let source = app
        .world()
        .resource::<DocumentRegistry<UsdDocument>>()
        .host(doc)
        .expect("document")
        .document()
        .source();
    assert!(source.contains("ExistingEdit"));
    assert!(!source.contains("Chassis"));
}

#[test]
fn rejecting_a_proposal_removes_only_review_state() {
    let (mut app, doc) = proposal_test_app();
    app.world_mut().trigger(CreateUsdProposal {
        doc_id: doc,
        scope: UsdEditScope::Assembly,
        label: "Reject chassis".to_owned(),
        parent_gen: 0,
        ops: vec![proposal_test_op("Chassis")],
    });
    app.update();
    let proposal = app
        .world()
        .resource::<UsdEditSessions>()
        .for_document(doc)
        .next()
        .expect("proposal")
        .id;

    app.world_mut().trigger(ReviewUsdProposal {
        proposal,
        action: UsdProposalReviewAction::Reject,
    });
    app.update();

    assert!(app
        .world()
        .resource::<UsdEditSessions>()
        .proposal(proposal)
        .is_none());
    assert_eq!(
        app.world()
            .resource::<DocumentRegistry<UsdDocument>>()
            .host(doc)
            .expect("document")
            .generation(),
        0
    );
}

#[test]
fn closing_a_document_drops_its_review_session() {
    let (mut app, doc) = proposal_test_app();
    app.world_mut().trigger(CreateUsdProposal {
        doc_id: doc,
        scope: UsdEditScope::Assembly,
        label: "Close chassis review".to_owned(),
        parent_gen: 0,
        ops: vec![proposal_test_op("Chassis")],
    });
    app.update();
    assert_eq!(
        app.world()
            .resource::<UsdEditSessions>()
            .for_document(doc)
            .count(),
        1
    );

    app.world_mut()
        .trigger(lunco_doc_bevy::DocumentClosed::local(doc));
    app.update();
    assert_eq!(
        app.world()
            .resource::<UsdEditSessions>()
            .for_document(doc)
            .count(),
        0
    );
}

#[test]
fn file_discard_invalidates_review_before_source_read_completes() {
    let temp = tempfile::tempdir().expect("discard source directory");
    let path = temp.path().join("Assembly.usda");
    let source = "#usda 1.0\ndef Xform \"Assembly\" {}\n";
    std::fs::write(&path, source).expect("discard source");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(UsdCommandsPlugin);
    install_command_result_resources(&mut app);
    app.update();
    let doc = app
        .world_mut()
        .resource_mut::<DocumentRegistry<UsdDocument>>()
        .open_file(path.display().to_string(), source.to_owned())
        .0;
    app.update();

    app.world_mut().trigger(CreateUsdProposal {
        doc_id: doc,
        scope: UsdEditScope::Assembly,
        label: "Discard chassis review".to_owned(),
        parent_gen: 0,
        ops: vec![proposal_test_op("Chassis")],
    });
    app.update();
    assert_eq!(
        app.world()
            .resource::<UsdEditSessions>()
            .for_document(doc)
            .count(),
        1
    );

    app.world_mut().trigger(DiscardDocument { doc_id: doc });
    app.update();
    assert_eq!(
        app.world()
            .resource::<UsdEditSessions>()
            .for_document(doc)
            .count(),
        0
    );
}

#[test]
fn fork_and_discard_use_document_lifecycle_commands() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(UsdCommandsPlugin);
    install_command_result_resources(&mut app);
    app.update();

    let source = {
        let mut registry = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>();
        registry.allocate(
            "#usda 1.0\ndef Xform \"Rig\" { def Xform \"Chassis\" {} }\n".to_owned(),
            lunco_doc::PathlessOrigin::untitled("Source.usda"),
        )
    };
    app.update();

    app.world_mut().trigger(ForkDocument {
        source_doc_id: source,
        name: "Fork.usda".to_owned(),
    });
    app.update();
    let ids: Vec<_> = app
        .world()
        .resource::<DocumentRegistry<UsdDocument>>()
        .ids()
        .collect();
    assert_eq!(ids.len(), 2);
    let fork = *ids.iter().find(|id| **id != source).expect("fork id");
    let registry = app.world().resource::<DocumentRegistry<UsdDocument>>();
    assert_eq!(
        registry.host(fork).expect("fork host").document().source(),
        registry
            .host(source)
            .expect("source host")
            .document()
            .source()
    );
    assert!(registry
        .host(fork)
        .expect("fork host")
        .document()
        .origin()
        .is_untitled());

    app.world_mut().trigger(DiscardDocument { doc_id: fork });
    app.update();
    assert!(!app
        .world()
        .resource::<DocumentRegistry<UsdDocument>>()
        .contains(fork));
    assert!(app
        .world()
        .resource::<DocumentRegistry<UsdDocument>>()
        .contains(source));
}

fn wait_for_one_usd_document(app: &mut App) {
    for _ in 0..1_000 {
        app.update();
        if app
            .world()
            .resource::<DocumentRegistry<UsdDocument>>()
            .ids()
            .count()
            == 1
        {
            return;
        }
        std::thread::yield_now();
    }
}

#[test]
fn open_file_for_usd_path_creates_document() {
    // Write a tiny .usda to a tempfile we can resolve.
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join("lunco_usd_open_file_test.usda");
    std::fs::write(&tmp_path, "#usda 1.0\ndef Xform \"X\" {}\n").unwrap();

    // `UsdCommandsPlugin` now owns the whole open pipeline (observer +
    // PendingUsdLoads + drain) — no UI plugin needed. `MinimalPlugins`
    // supplies the `AsyncComputeTaskPool` the read runs on.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(UsdCommandsPlugin);
    app.update();

    app.world_mut().trigger(OpenFile {
        path: tmp_path.to_string_lossy().to_string(),
    });
    // Flush the queued world-command (spawns the async read task), then
    // wait for the actual document-allocation result.
    wait_for_one_usd_document(&mut app);

    let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
    assert_eq!(
        reg.ids().count(),
        1,
        "exactly one USD doc opened (no duplicate)"
    );

    let _ = std::fs::remove_file(&tmp_path);
}

#[test]
fn open_file_file_uri_creates_document() {
    let tmp_path = std::env::temp_dir().join("lunco_usd_open_file_uri_test.usda");
    std::fs::write(&tmp_path, "#usda 1.0\ndef Xform \"X\" {}\n").unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(UsdCommandsPlugin);
    app.update();

    app.world_mut().trigger(OpenFile {
        path: format!("file://{}", tmp_path.display()),
    });
    wait_for_one_usd_document(&mut app);

    assert_eq!(
        app.world()
            .resource::<DocumentRegistry<UsdDocument>>()
            .ids()
            .count(),
        1,
        "file:// USD paths must use the filesystem document reader"
    );
    let _ = std::fs::remove_file(&tmp_path);
}

#[test]
fn open_file_for_non_usd_path_is_noop() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(UsdCommandsPlugin);
    app.update();

    app.world_mut().trigger(OpenFile {
        path: "/tmp/some_model.mo".to_string(),
    });
    for _ in 0..5 {
        app.update();
    }

    let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
    assert_eq!(reg.ids().count(), 0, "non-USD path must not allocate");
}

#[test]
fn apply_usd_op_builds_a_rover_through_typed_command_bus() {
    use lunco_doc::Document;
    use lunco_usd_document::document::{LayerId, UsdOp};

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(UsdCommandsPlugin);
    install_command_result_resources(&mut app);
    app.update();

    // Allocate a blank document.
    let doc_id = {
        let mut reg = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>();
        reg.allocate(
            "#usda 1.0\n(\n    metersPerUnit = 1\n)\n".to_string(),
            lunco_doc::PathlessOrigin::untitled("UntitledRover.usda"),
        )
    };
    app.update();

    // Drive a sequence of ApplyUsdOp commands — same path UI
    // toolbars and the HTTP API will use.
    let ops = [
        UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/".into(),
            name: "Rover".into(),
            type_name: Some("Xform".into()),
            reference: None,
            reference_prim_path: None,
        },
        UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/Rover".into(),
            name: "Body".into(),
            type_name: Some("Cube".into()),
            reference: None,
            reference_prim_path: None,
        },
        UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/Rover".into(),
            name: "WheelFL".into(),
            type_name: Some("Cube".into()),
            reference: None,
            reference_prim_path: None,
        },
        UsdOp::SetTranslate {
            edit_target: LayerId::root(),
            path: "/Rover/WheelFL".into(),
            value: [1.0, 0.0, 1.0],
        },
    ];
    for op in ops {
        app.world_mut().trigger(ApplyUsdOp {
            doc_id,
            parent_gen: None,
            op,
        });
        app.update();
    }
    // One more tick to flush any final queued world commands.
    app.update();

    use lunco_usd_data::usd_data::UsdDataExt;
    use openusd::sdf::Path as SdfPath;
    let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
    let host = reg.host(doc_id).expect("doc still alive");
    // Assert on the canonical data (the document is data-canonical now;
    // exact serialized-text formatting is openusd's business, not ours).
    let data = host.document().data();
    // `UsdDataExt` on purpose: this asserts what the ops AUTHORED into the
    // document layer, not what a stage composes out of it.
    assert_eq!(
        data.prim_type_name(&SdfPath::new("/Rover").unwrap())
            .as_deref(),
        Some("Xform")
    );
    assert_eq!(
        data.prim_type_name(&SdfPath::new("/Rover/Body").unwrap())
            .as_deref(),
        Some("Cube")
    );
    assert_eq!(
        data.prim_type_name(&SdfPath::new("/Rover/WheelFL").unwrap())
            .as_deref(),
        Some("Cube")
    );
    assert_eq!(
        data.prim_attribute_value::<[f64; 3]>(
            &SdfPath::new("/Rover/WheelFL").unwrap(),
            "xformOp:translate"
        ),
        Some([1.0, 0.0, 1.0])
    );
    // Generation advanced once per op.
    assert_eq!(host.document().generation(), 4);
}

/// Phase A1: every `ApplyUsdOp` that lands records one **lossless**
/// `EntryKind::Op` into the canonical Twin journal — the recorded op
/// deserializes back to the exact `UsdOp` (not a hand summary), and a
/// real `UsdOp` inverse rides alongside it.
#[test]
fn apply_usd_op_records_lossless_journal_entries() {
    use lunco_twin_journal::{DomainKind, EntryKind};
    use lunco_usd_document::document::{LayerId, UsdOp};

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(UsdCommandsPlugin);
    install_command_result_resources(&mut app);
    // The Twin-journal plugin isn't part of `UsdCommandsPlugin`; install
    // the resource directly so the apply funnel has somewhere to record.
    app.insert_resource(lunco_doc_bevy::JournalResource::default());
    app.update();

    let doc_id = {
        let mut reg = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>();
        reg.allocate(
            "#usda 1.0\n".to_string(),
            lunco_doc::PathlessOrigin::untitled("UntitledJournal.usda"),
        )
    };
    app.update();

    let forward_ops = [
        UsdOp::AddPrim {
            edit_target: LayerId::root(),
            parent_path: "/".into(),
            name: "Rover".into(),
            type_name: Some("Xform".into()),
            reference: None,
            reference_prim_path: None,
        },
        UsdOp::SetTranslate {
            edit_target: LayerId::root(),
            path: "/Rover".into(),
            value: [2.0, 0.0, 5.0],
        },
    ];
    for op in forward_ops.clone() {
        app.world_mut().trigger(ApplyUsdOp {
            doc_id,
            parent_gen: None,
            op,
        });
        app.update();
    }
    app.update();

    let journal = app.world().resource::<lunco_doc_bevy::JournalResource>();
    journal.with_read(|j| {
        let ops: Vec<_> = j
            .entries_for_doc(doc_id)
            .filter_map(|e| match &e.kind {
                EntryKind::Op {
                    domain,
                    op,
                    inverse,
                } => Some((domain.clone(), op.clone(), inverse.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(ops.len(), 2, "one Op entry recorded per applied UsdOp");
        for (i, (domain, op_val, inv_val)) in ops.iter().enumerate() {
            assert_eq!(*domain, DomainKind::Usd);
            // Lossless: the recorded op deserializes back to the exact UsdOp.
            let decoded: UsdOp =
                serde_json::from_value(op_val.clone()).expect("recorded op round-trips to UsdOp");
            assert_eq!(format!("{decoded:?}"), format!("{:?}", forward_ops[i]));
            // The inverse is a real UsdOp too. Phase C3 records TYPED
            // inverses where exact: AddPrim of a brand-new prim inverts to
            // a RemovePrim; SetTranslate that synthesizes `xformOpOrder`
            // falls back to a coarse full-source ReplaceSource snapshot.
            let inv: UsdOp = serde_json::from_value(inv_val.clone())
                .expect("recorded inverse round-trips to UsdOp");
            match i {
                0 => assert!(
                    matches!(inv, UsdOp::RemovePrim { .. }),
                    "AddPrim of a new prim inverts to a typed RemovePrim, got {inv:?}"
                ),
                1 => assert!(
                    matches!(inv, UsdOp::ReplaceSource { .. }),
                    "SetTranslate inverts to a coarse ReplaceSource, got {inv:?}"
                ),
                _ => unreachable!(),
            }
        }
    });
}

#[test]
fn new_document_with_usd_kind_creates_untitled() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(UsdCommandsPlugin);
    app.update();

    app.world_mut().trigger(NewDocument {
        kind: USD_DOCUMENT_KIND.to_string(),
    });
    app.update();
    app.update();

    let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
    assert_eq!(reg.ids().count(), 1);
    let id = reg.ids().next().unwrap();
    assert!(reg.host(id).unwrap().document().origin().is_untitled());
}

#[test]
fn save_as_untitled_usd_writes_source_and_rebinds_origin() {
    let tmp = tempfile::tempdir().expect("save destination");
    let target = tmp.path().join("scene.usda");
    let source = "#usda 1.0\ndef Xform \"World\" {}\n";

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(UsdCommandsPlugin);
    app.update();

    let doc = {
        let mut registry = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>();
        registry.allocate(
            source.to_string(),
            lunco_doc::PathlessOrigin::untitled("UntitledStage.usda"),
        )
    };
    app.world_mut().trigger(lunco_doc_bevy::SaveAsDocument {
        doc_id: doc,
        path: target.display().to_string(),
    });
    app.update();

    let registry = app.world().resource::<DocumentRegistry<UsdDocument>>();
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        registry.host(doc).unwrap().document().source()
    );
    assert_eq!(
        registry
            .host(doc)
            .unwrap()
            .document()
            .origin()
            .canonical_path(),
        Some(target.as_path())
    );
    assert!(!registry.host(doc).unwrap().document().is_dirty());
}
