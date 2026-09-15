//! Integration coverage for the public USD inspection and assembly query API.
//!
//! These providers are consumed by Rhai, HTTP, MCP, and headless hosts. Keep
//! their observable contract outside `commands.rs`; the command implementation
//! should not be recompiled as part of this integration target's test code.

use bevy::prelude::World;
use lunco_api::queries::ApiQueryProvider;
use lunco_doc::PathlessOrigin;
use lunco_doc_bevy::DocumentRegistry;
use lunco_usd_core::edit_session::{validate_proposal, UsdEditScope, UsdEditSessions};
use lunco_usd_document::document::{LayerId, UsdDocument, UsdOp};
use lunco_usd_queries::{
    InspectUsdDocumentProvider, InspectUsdEditSessionProvider, ResolveUsdTargetProvider,
    SyncUsdDocumentProvider,
};

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

#[test]
fn inspect_query_requires_explicit_document_and_reads_composed_prim() {
    let mut world = World::new();
    let mut registry = DocumentRegistry::<UsdDocument>::default();
    let doc = registry.allocate(
        "#usda 1.0\ndef Xform \"Rig\" { def Xform \"Chassis\" {} }\n".to_owned(),
        PathlessOrigin::untitled("Inspect.usda"),
    );
    world.insert_resource(registry);

    let missing = InspectUsdDocumentProvider.execute(&world, &serde_json::json!({}));
    assert!(matches!(
        missing,
        lunco_api::schema::ApiResponse::Error { .. }
    ));
    let inspected = InspectUsdDocumentProvider.execute(
        &world,
        &serde_json::json!({ "doc_id": doc.raw(), "path": "/Rig/Chassis" }),
    );
    let lunco_api::schema::ApiResponse::Ok { data: Some(data) } = inspected else {
        panic!("inspection query must return structured data");
    };
    assert_eq!(data["doc_id"], serde_json::json!(doc));
    assert_eq!(data["prim"]["exists"], serde_json::json!(true));
    assert_eq!(data["prim"]["type"], serde_json::json!("Xform"));
}

#[test]
fn inspect_edit_session_query_returns_typed_review_state() {
    let mut world = World::new();
    let mut registry = DocumentRegistry::<UsdDocument>::default();
    let doc = registry.allocate(
        "#usda 1.0\ndef Xform \"Assembly\" {}\n".to_owned(),
        PathlessOrigin::untitled("Session.usda"),
    );
    let operation = proposal_test_op("Chassis");
    let document = registry.host(doc).expect("document").document();
    let validation = validate_proposal(
        document,
        UsdEditScope::Assembly,
        0,
        std::slice::from_ref(&operation),
    );
    let mut sessions = UsdEditSessions::default();
    let proposal = sessions.insert(
        doc,
        UsdEditScope::Assembly,
        "Add chassis".to_owned(),
        0,
        validation,
        vec![operation],
    );
    world.insert_resource(registry);
    world.insert_resource(sessions);

    let response =
        InspectUsdEditSessionProvider.execute(&world, &serde_json::json!({ "doc_id": doc.raw() }));
    let lunco_api::schema::ApiResponse::Ok { data: Some(data) } = response else {
        panic!("edit-session query must return structured data");
    };
    assert_eq!(data["doc_id"], serde_json::json!(doc));
    assert_eq!(data["generation"], serde_json::json!(0));
    assert_eq!(data["proposals"][0]["id"], serde_json::json!(proposal));
    assert_eq!(data["proposals"][0]["scope"], serde_json::json!("assembly"));
    assert_eq!(data["proposals"][0]["state"], serde_json::json!("pending"));
    assert_eq!(data["proposals"][0]["stale"], serde_json::json!(false));
    assert_eq!(data["proposals"][0]["ops"].as_array().unwrap().len(), 1);
}

#[test]
fn assembly_queries_resolve_local_targets_and_reject_future_sync_cursors() {
    let mut world = World::new();
    let mut registry = DocumentRegistry::<UsdDocument>::default();
    let doc = registry.allocate(
        "#usda 1.0\ndef Xform \"Rig\" { def Xform \"Chassis\" {} }\n".to_owned(),
        PathlessOrigin::untitled("Assembly.usda"),
    );
    registry
        .apply(
            doc,
            UsdOp::AddPrim {
                edit_target: LayerId::root(),
                parent_path: "/Rig".to_owned(),
                name: "Wheel".to_owned(),
                type_name: Some("Xform".to_owned()),
                reference: None,
                reference_prim_path: None,
            },
        )
        .expect("local assembly edit");
    world.insert_resource(registry);

    let resolved = ResolveUsdTargetProvider.execute(
        &world,
        &serde_json::json!({
            "doc_id": doc.raw(),
            "path": "/Rig/Wheel",
            "edit_target": "@runtime@"
        }),
    );
    let lunco_api::schema::ApiResponse::Ok { data: Some(data) } = resolved else {
        panic!("local assembly target must resolve");
    };
    assert_eq!(data["status"], serde_json::json!("resolved"));
    assert_eq!(data["source"], serde_json::json!("document_layers"));
    assert_eq!(data["authored_in_document"], serde_json::json!(true));

    let future = SyncUsdDocumentProvider.execute(
        &world,
        &serde_json::json!({ "doc_id": doc.raw(), "since_generation": 99 }),
    );
    assert!(matches!(
        future,
        lunco_api::schema::ApiResponse::Error { code: 409, .. }
    ));

    let delta = SyncUsdDocumentProvider.execute(
        &world,
        &serde_json::json!({ "doc_id": doc.raw(), "since_generation": 0 }),
    );
    let lunco_api::schema::ApiResponse::Ok { data: Some(delta) } = delta else {
        panic!("sync cursor must return an op delta");
    };
    assert_eq!(delta["kind"], serde_json::json!("delta"));
    assert_eq!(delta["from_generation"], serde_json::json!(0));
    assert_eq!(delta["to_generation"], serde_json::json!(1));
    assert_eq!(delta["ops"].as_array().expect("ops array").len(), 1);
}

#[test]
fn authored_target_under_composed_arc_resolves_before_stage_projection() {
    let mut world = World::new();
    let mut registry = DocumentRegistry::<UsdDocument>::default();
    let doc = registry.allocate(
        r#"#usda 1.0
def Xform "Site" (
    references = @site.usda@
)
{
    def Xform "Route" {
        def Xform "W0" {}
    }
}
"#
        .to_owned(),
        PathlessOrigin::untitled("ProjectedRoute.usda"),
    );
    world.insert_resource(registry);

    let resolved = ResolveUsdTargetProvider.execute(
        &world,
        &serde_json::json!({
            "doc_id": doc.raw(),
            "path": "/Site/Route/W0",
            "edit_target": "@root@"
        }),
    );
    let lunco_api::schema::ApiResponse::Ok { data: Some(data) } = resolved else {
        panic!("an authored local target must resolve before canonical projection catches up");
    };
    assert_eq!(data["status"], serde_json::json!("resolved"));
    assert_eq!(data["source"], serde_json::json!("document_layers"));
    assert_eq!(data["authored_in_document"], serde_json::json!(true));
    assert_eq!(data["under_arc"], serde_json::json!(true));
    assert_eq!(data["edit_scope"], serde_json::json!("authored_layer"));
}

#[test]
fn sync_query_returns_a_complete_snapshot_after_the_op_ring_expires() {
    let mut world = World::new();
    let mut registry = DocumentRegistry::<UsdDocument>::default();
    let doc = registry.allocate(
        "#usda 1.0\ndef Xform \"Assembly\" {}\n".to_owned(),
        PathlessOrigin::untitled("Snapshot.usda"),
    );
    for index in 0..257 {
        registry
            .apply(
                doc,
                UsdOp::AddPrim {
                    edit_target: LayerId::root(),
                    parent_path: "/Assembly".to_owned(),
                    name: format!("Part{index}"),
                    type_name: Some("Xform".to_owned()),
                    reference: None,
                    reference_prim_path: None,
                },
            )
            .expect("assembly edit in history window");
    }
    world.insert_resource(registry);

    let response = SyncUsdDocumentProvider.execute(
        &world,
        &serde_json::json!({ "doc_id": doc.raw(), "since_generation": 0 }),
    );
    let lunco_api::schema::ApiResponse::Ok {
        data: Some(snapshot),
    } = response
    else {
        panic!("expired cursor must receive a resync snapshot");
    };
    assert_eq!(snapshot["kind"], serde_json::json!("snapshot"));
    assert_eq!(
        snapshot["reason"],
        serde_json::json!("history_window_exceeded")
    );
    assert_eq!(snapshot["generation"], serde_json::json!(257));
    assert!(snapshot["layers"]["root"]["source"]
        .as_str()
        .is_some_and(|source| source.contains("Part256")));
}
