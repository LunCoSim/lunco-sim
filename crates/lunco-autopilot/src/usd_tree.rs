//! Missions authored as **BT.CPP XML + USD waypoint prims**.
//!
//! The two halves answer different questions, so each is stored in the format that
//! is actually good at it:
//!
//! - **The tree's topology** — sequences, decorators, which tool fires where — is
//!   BehaviorTree.CPP v4 XML ([`crate::btcpp_xml`]). Portable: Groot2 edits it, ROS/
//!   Nav2 runs it.
//! - **The mission's geometry** — where the waypoints *are* — is USD prims. A
//!   waypoint is a real prim, so it is selectable, draggable with the ordinary
//!   transform gizmo, deletable, journaled, undoable, persisted, and replicated by
//!   the machinery that already serves every other prim.
//!
//! The XML's spatial leaves **reference** the prims by path instead of baking
//! coordinates — which is how BT.CPP is meant to be used anyway (leaves read ports,
//! not constants):
//!
//! ```xml
//! <Repeat><Sequence>
//!   <Action ID="drive_to" target="/World/Behaviors/RoverPatrol/wp0"/>
//!   <Action ID="run_tool" tool="science::take_photo"/>
//!   <Action ID="drive_to" target="/World/Behaviors/RoverPatrol/wp1"/>
//! </Sequence></Repeat>
//! ```
//!
//! ```usda
//! def Xform "Rover" {
//!     custom string lunco:vessel = "true"
//!     custom string lunco:behaviorPath = "behaviors/rover_patrol.xml"   # or inline lunco:behavior
//! }
//! def Scope "Behaviors" { def "RoverPatrol" {
//!     def Xform "wp0" (prepend references = @vessels/markers/waypoint.usda@) {
//!         double3 xformOp:translate = (10, 0, 3)
//!         uniform token[] xformOpOrder = ["xformOp:translate"]
//!     }
//! }}
//! ```
//!
//! `lunco:behavior` (inline) / `lunco:behaviorPath` (asset) mirror the established
//! `lunco:script` / `lunco:scriptPath` pair exactly, inline winning over the file.
//!
//! ## Waypoints are not children of the vessel
//!
//! A route is in WORLD space. Parenting the waypoints under the rover would make
//! them ride along as it drives — the route would chase the vehicle. They live in a
//! sibling scope, and the XML names them by path.
//!
//! ## Resolution happens at COMPILE time, not tick time
//!
//! [`compile_behavior_xml`] resolves each `target` prim path to that prim's live
//! `GlobalTransform` and bakes the coordinates into the compiled tree — then
//! recompiles whenever a referenced prim MOVES. So dragging a pin in the viewport
//! re-routes the rover, and the hot path (`drive_autopilots`) stays a plain
//! coordinate chase with no per-tick lookups.
//!
//! `BehaviorSpec` therefore needs no prim-path variant: the reference exists only in
//! the XML/JSON intermediate, and is gone by the time a tree is built.

use crate::{Autopilot, AutopilotBehavior, AutopilotBehaviorSpec, BehaviorSpec};
use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use serde_json::Value;

/// The XML text of a vessel's behaviour tree — inline `lunco:behavior`, or the
/// loaded contents of `lunco:behaviorPath`. Stamped on the VESSEL entity by the USD
/// bridge (`lunco-usd-sim`).
#[derive(Component, Debug, Clone)]
pub struct BehaviorXml(pub String);

/// A vessel whose `lunco:behaviorPath` names a `.xml` asset still being loaded.
/// [`load_behavior_xml_assets`] swaps it for [`BehaviorXml`] once the asset lands.
#[derive(Component, Debug, Clone)]
pub struct BehaviorXmlPath(pub String);

/// Handle to the in-flight `.xml` asset for a [`BehaviorXmlPath`].
#[derive(Component)]
pub struct BehaviorXmlHandle(pub Handle<BehaviorXmlAsset>);

/// Prim path → live entity, for every spatial leaf the XML references. Written by
/// the USD bridge (which owns prim-path resolution); read here to bake coordinates.
///
/// A path that does not resolve is simply absent — [`compile_behavior_xml`] refuses
/// to compile a tree with a dangling target rather than silently driving to the
/// origin.
#[derive(Component, Debug, Clone, Default)]
pub struct TargetBindings(pub HashMap<String, Entity>);

/// Raw text of a `.xml` behaviour tree — the file-backed twin of inline
/// `lunco:behavior`, so a mission stays an editable, hot-reloadable file that Groot2
/// can open.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct BehaviorXmlAsset {
    /// Raw BT.CPP v4 XML. UTF-8.
    pub text: String,
}

#[derive(Default, TypePath)]
pub struct BehaviorXmlLoader;

impl AssetLoader for BehaviorXmlLoader {
    type Asset = BehaviorXmlAsset;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(BehaviorXmlAsset {
            text: String::from_utf8(bytes)?,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["btxml"]
    }
}

/// Kick off the asset load for each `lunco:behaviorPath`, and swap the loaded text in
/// as [`BehaviorXml`]. Routed through `AssetServer` (never `std::fs`) so it works on
/// wasm — same rule as the `.rhai` scenario loader.
pub fn load_behavior_xml_assets(
    q_pending: Query<(Entity, &BehaviorXmlPath), Without<BehaviorXmlHandle>>,
    q_loading: Query<(Entity, &BehaviorXmlHandle)>,
    assets: Res<Assets<BehaviorXmlAsset>>,
    server: Res<AssetServer>,
    mut commands: Commands,
) {
    for (e, path) in q_pending.iter() {
        commands
            .entity(e)
            .try_insert(BehaviorXmlHandle(server.load(path.0.clone())));
    }
    for (e, handle) in q_loading.iter() {
        let Some(asset) = assets.get(&handle.0) else {
            continue; // still loading
        };
        commands
            .entity(e)
            .try_insert(BehaviorXml(asset.text.clone()))
            .remove::<BehaviorXmlPath>()
            .remove::<BehaviorXmlHandle>();
    }
}

/// Every prim path a tree's spatial leaves reference — what the USD bridge must
/// resolve into [`TargetBindings`].
///
/// A `target` that parses as a coordinate triple (`"10;0;3"`, the plain BT.CPP form)
/// is NOT a prim path and is skipped: a tree with baked coordinates still runs, it
/// just has no draggable pins.
pub fn target_paths(xml: &str) -> Vec<String> {
    let Ok(value) = crate::btcpp_xml::xml_to_value(xml) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_target_paths(&value, &mut out);
    out
}

fn collect_target_paths(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            if let Some(Value::String(s)) = map.get("target") {
                if s.starts_with('/') {
                    out.push(s.clone());
                }
            }
            for child in map.values() {
                collect_target_paths(child, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_target_paths(i, out)),
        _ => {}
    }
}

/// Append a `drive_to` leaf referencing the waypoint prim at `prim_path` to a
/// vessel's mission, returning the new BT.CPP XML.
///
/// `xml` is the vessel's current tree, or `None` for a vessel with no mission yet —
/// in which case the canonical patrol shell is created:
/// `forever(sequence[drive_to])`.
///
/// This is the ONE place that edits a mission's topology from the editor, and it is
/// deliberately conservative: it appends only to the plain `forever → sequence`
/// patrol shape it knows how to extend. A hand-authored tree of any other shape is
/// left alone (`Err`) — the user edits that in Groot2 or by hand, and the editor does
/// not get to silently restructure it.
pub fn append_waypoint_leaf(xml: Option<&str>, prim_path: &str) -> Result<String, String> {
    let leaf = serde_json::json!({ "kind": "drive_to", "target": prim_path });

    let mut root = match xml {
        // No mission yet → the patrol shell.
        None => serde_json::json!({
            "kind": "forever",
            "child": { "kind": "sequence", "children": [] }
        }),
        Some(text) => crate::btcpp_xml::xml_to_value(text)?,
    };

    // Reach into `forever.child.children` — the leg list of a patrol.
    let legs = root
        .get_mut("child")
        .filter(|c| c.get("kind").and_then(|k| k.as_str()) == Some("sequence"))
        .and_then(|c| c.get_mut("children"))
        .and_then(|c| c.as_array_mut())
        .ok_or_else(|| {
            "mission is not a plain forever(sequence[…]) patrol; edit its XML directly \
             (the editor will not restructure a hand-authored tree)"
                .to_string()
        })?;
    legs.push(leaf);

    crate::btcpp_xml::value_to_xml(&root)
}

/// Remove a `drive_to` leaf referencing the waypoint prim at `prim_path` from a
/// vessel's mission, returning the new BT.CPP XML.
pub fn remove_waypoint_leaf(xml: &str, prim_path: &str) -> Result<String, String> {
    let mut root = crate::btcpp_xml::xml_to_value(xml)?;

    // Reach into `forever.child.children` — the leg list of a patrol.
    let legs = root
        .get_mut("child")
        .filter(|c| c.get("kind").and_then(|k| k.as_str()) == Some("sequence"))
        .and_then(|c| c.get_mut("children"))
        .and_then(|c| c.as_array_mut())
        .ok_or_else(|| {
            "mission is not a plain forever(sequence[…]) patrol"
                .to_string()
        })?;

    legs.retain(|child| {
        child.get("target").and_then(|t| t.as_str()) != Some(prim_path)
    });

    crate::btcpp_xml::value_to_xml(&root)
}

/// Replace every prim-path `target` with the prim's live world position. Returns the
/// paths that could not be resolved — a tree naming a deleted waypoint must not
/// compile (it would drive to the origin).
fn bake_targets(
    v: &mut Value,
    bindings: &TargetBindings,
    q_gt: &Query<&GlobalTransform>,
    missing: &mut Vec<String>,
) {
    match v {
        Value::Object(map) => {
            let resolved = match map.get("target") {
                Some(Value::String(s)) => {
                    if s.starts_with('/') {
                        match bindings.0.get(s.as_str()).and_then(|e| q_gt.get(*e).ok()) {
                            Some(gt) => {
                                let p = gt.translation();
                                Some(serde_json::json!([p.x, p.y, p.z]))
                            }
                            None => {
                                missing.push(s.clone());
                                None
                            }
                        }
                    } else {
                        let parts: Vec<&str> = s.split(';').collect();
                        if parts.len() == 3 {
                            if let (Ok(x), Ok(y), Ok(z)) = (
                                parts[0].trim().parse::<f32>(),
                                parts[1].trim().parse::<f32>(),
                                parts[2].trim().parse::<f32>(),
                            ) {
                                Some(serde_json::json!([x, y, z]))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                }
                _ => None,
            };
            if let Some(pos) = resolved {
                map.insert("target".into(), pos);
            }
            for child in map.values_mut() {
                bake_targets(child, bindings, q_gt, missing);
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|i| bake_targets(i, bindings, q_gt, missing)),
        _ => {}
    }
}

/// Compile each vessel's XML tree — resolving its prim-path targets to live world
/// positions — into the derived [`AutopilotBehaviorSpec`], and hot-swap the running
/// [`AutopilotBehavior`] so an edit takes effect immediately.
///
/// Change-gated (§7): recompiles when the XML changes, when the bindings change, or
/// when any entity MOVES — the last is what makes dragging a waypoint pin re-route
/// the rover. The move gate is deliberately coarse (`Changed<GlobalTransform>` over
/// all entities) but cheap: it only costs a rebuild for vessels that actually carry a
/// tree, and a moving *vessel* re-baking its own static targets is idempotent.
pub fn compile_behavior_xml(
    q_vessels: Query<(Entity, &BehaviorXml, Option<&TargetBindings>)>,
    q_gt: Query<&GlobalTransform>,
    q_autopilots: Query<(Entity, &Autopilot)>,
    moved: Query<
        Entity,
        Or<(
            Changed<BehaviorXml>,
            Changed<TargetBindings>,
            Changed<GlobalTransform>,
        )>,
    >,
    mut commands: Commands,
) {
    if q_vessels.is_empty() || moved.is_empty() {
        return;
    }
    for (vessel, xml, bindings) in q_vessels.iter() {
        let empty = TargetBindings::default();
        let bindings = bindings.unwrap_or(&empty);

        let mut value = match crate::btcpp_xml::xml_to_value(&xml.0) {
            Ok(v) => v,
            Err(err) => {
                warn!("[autopilot/usd] behaviour XML for {vessel:?} is not valid BT.CPP: {err}");
                continue;
            }
        };

        let mut missing = Vec::new();
        bake_targets(&mut value, bindings, &q_gt, &mut missing);
        if !missing.is_empty() {
            // Dangling reference: a waypoint prim the tree names has been deleted (or
            // has not spawned yet). Keep the last good tree — compiling this one would
            // send the rover to the world origin.
            debug!(
                "[autopilot/usd] behaviour tree for {vessel:?} references unresolved waypoint(s) \
                 {missing:?}; not recompiling"
            );
            continue;
        }

        let spec: BehaviorSpec = match serde_json::from_value(value) {
            Ok(s) => s,
            Err(err) => {
                warn!("[autopilot/usd] behaviour tree for {vessel:?} is not a valid spec: {err}");
                continue;
            }
        };

        // The spec on the vessel is DERIVED — a projection of the XML + the prims.
        commands
            .entity(vessel)
            .try_insert(AutopilotBehaviorSpec(spec.clone()));
        // Live hot-swap: if an autopilot is already driving, re-point it at the edited
        // route without a disengage/re-engage cycle.
        if let Some((actor, _)) = q_autopilots.iter().find(|(_, ap)| ap.vessel == vessel) {
            commands
                .entity(actor)
                .try_insert(AutopilotBehavior::new(&spec));
        }
    }
}
