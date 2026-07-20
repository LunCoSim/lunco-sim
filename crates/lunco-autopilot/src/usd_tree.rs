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
//!     def LunCoProgram "Mission" {
//!         uniform asset info:sourceAsset = @behaviors/rover_patrol.xml@   # or inline info:sourceCode
//!     }
//! }
//! def Scope "Behaviors" { def "RoverPatrol" {
//!     def Xform "wp0" (prepend references = @vessels/markers/waypoint.usda@) {
//!         double3 xformOp:translate = (10, 0, 3)
//!         uniform token[] xformOpOrder = ["xformOp:translate"]
//!     }
//! }}
//! ```
//!
//! A mission is BOLTED ON: it is a `LunCoProgram` child prim (conventionally named
//! `Mission`) carrying the standard UsdShade-style source properties —
//! `info:sourceCode` (inline) / `info:sourceAsset` (file), inline winning over the
//! file. Delete the prim and the behaviour is gone. The engine is chosen by the
//! source's EXTENSION: `.xml` → BT.CPP, exactly as `.mo` → Modelica and `.rhai` →
//! script already work. There is no behaviour-specific schema and no separate
//! "which child is the tree" pointer.
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
use bevy::math::DVec3;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use serde_json::Value;

/// The XML text of a vessel's behaviour tree — inline `info:sourceCode` on the
/// vessel's `LunCoProgram` mission child, or the loaded contents of that prim's
/// `info:sourceAsset`. Stamped on the VESSEL entity by the USD bridge
/// (`lunco-usd-sim`).
#[derive(Component, Debug, Clone)]
pub struct BehaviorXml(pub String);

/// A vessel whose mission prim's `info:sourceAsset` names a `.xml` asset still being
/// loaded.
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

/// Runtime-only set of waypoint coordinate keys (`"x;y;z"`, verbatim from the leg's
/// `target`) this vessel has already reached this session.
///
/// LIVE-SCENE STATE — deliberately NOT authored to USD. It greys the visited pin and
/// strips the leg from the **compiled** tree so the rover advances, but the on-disk
/// mission `info:sourceCode` XML keeps every leg. It resets on reload (the component just
/// starts empty). Kept out of the XML on purpose: dropping a new waypoint re-authors
/// the whole XML through `ApplyUsdOp`, which would otherwise bake this transient
/// "visited" flag into the saved `.usda`. [`compile_behavior_xml`] reads it to strip
/// reached legs; `sync_waypoint_visuals` reads it to grey them.
#[derive(Component, Debug, Clone, Default)]
pub struct ReachedWaypoints(pub std::collections::HashSet<String>);

/// Raw text of a `.xml` behaviour tree — the file-backed twin of inline
/// `info:sourceCode`, so a mission stays an editable, hot-reloadable file that Groot2
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

/// Kick off the asset load for each mission `info:sourceAsset`, and swap the loaded text in
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

/// Format a world point as the `target="x;y;z"` coord key the editor authors.
/// The single spelling of a waypoint coordinate — keep capture, move and insert
/// using this so a coord key always round-trips and matches by string.
pub fn format_coord_target(p: DVec3) -> String {
    format!("{:.6};{:.6};{:.6}", p.x, p.y, p.z)
}

/// Borrow the leg list of a plain `forever(sequence[…])` patrol, for the editor
/// helpers below. Shared so every mutation agrees on the shape it will restructure
/// (and refuses to touch anything else).
fn legs_mut(root: &mut Value) -> Result<&mut Vec<Value>, String> {
    root.get_mut("child")
        .filter(|c| c.get("kind").and_then(|k| k.as_str()) == Some("sequence"))
        .and_then(|c| c.get_mut("children"))
        .and_then(|c| c.as_array_mut())
        .ok_or_else(|| "mission is not a plain forever(sequence[…]) patrol".to_string())
}

/// Move a waypoint: repoint the first `drive_to` leg matching `old_target` at
/// `new_target`, keeping its position in the sequence (and its dwell).
pub fn set_waypoint_target(xml: &str, old_target: &str, new_target: &str) -> Result<String, String> {
    let mut root = crate::btcpp_xml::xml_to_value(xml)?;
    let legs = legs_mut(&mut root)?;
    let leg = legs
        .iter_mut()
        .find(|l| l.get("target").and_then(|t| t.as_str()) == Some(old_target))
        .ok_or_else(|| format!("target '{old_target}' not found"))?;
    leg.as_object_mut()
        .ok_or_else(|| "leg is not an object".to_string())?
        .insert("target".into(), Value::String(new_target.to_string()));
    crate::btcpp_xml::value_to_xml(&root)
}

/// Insert a new `drive_to` leg at `new_target` directly AFTER the leg matching
/// `after_target` — so a waypoint can be added mid-route, not just appended.
pub fn insert_waypoint_after(xml: &str, after_target: &str, new_target: &str) -> Result<String, String> {
    let mut root = crate::btcpp_xml::xml_to_value(xml)?;
    let legs = legs_mut(&mut root)?;
    let at = legs
        .iter()
        .position(|l| l.get("target").and_then(|t| t.as_str()) == Some(after_target))
        .ok_or_else(|| format!("target '{after_target}' not found"))?;
    legs.insert(at + 1, serde_json::json!({ "kind": "drive_to", "target": new_target }));
    crate::btcpp_xml::value_to_xml(&root)
}

/// Set (or clear, with `0`) a waypoint's dwell — the seconds the rover holds there
/// before departing. Stored as a `dwell` attribute on the leg;
/// [`expand_editor_route_in_place`] turns it into a real `wait` node at compile time.
pub fn set_waypoint_dwell(xml: &str, target: &str, dwell: f64) -> Result<String, String> {
    let mut root = crate::btcpp_xml::xml_to_value(xml)?;
    let legs = legs_mut(&mut root)?;
    let leg = legs
        .iter_mut()
        .find(|l| l.get("target").and_then(|t| t.as_str()) == Some(target))
        .ok_or_else(|| format!("target '{target}' not found"))?;
    let map = leg.as_object_mut().ok_or_else(|| "leg is not an object".to_string())?;
    if dwell > 0.0 {
        map.insert("dwell".into(), serde_json::json!(dwell));
    } else {
        map.remove("dwell");
    }
    crate::btcpp_xml::value_to_xml(&root)
}

/// Read a waypoint's authored dwell (seconds), if any. `0`/absent → no dwell.
pub fn waypoint_dwell(xml: &str, target: &str) -> Option<f64> {
    let root = crate::btcpp_xml::xml_to_value(xml).ok()?;
    root.get("child")?
        .get("children")?
        .as_array()?
        .iter()
        .find(|l| l.get("target").and_then(|t| t.as_str()) == Some(target))
        .and_then(|l| l.get("dwell"))
        .and_then(|d| d.as_f64())
}

/// Whether the route is authored as a **smooth** (Catmull-Rom) path rather than
/// straight legs.
///
/// Conceptually route-level, but stored as a `smooth` attribute on each `drive_to`
/// LEG rather than on the `Sequence`: the BT.CPP parser deliberately drops unknown
/// attributes from known control elements (that's what keeps Groot's `name`/`_uid`
/// decorations out of the spec), so a flag on `<Sequence>` would not survive the XML
/// round-trip. The legs are `<Action ID="drive_to">`, whose ports ARE preserved.
/// The whole route is one path, so the flag is read from the first leg and written to
/// all of them.
pub fn route_is_smooth(xml: &str) -> bool {
    crate::btcpp_xml::xml_to_value(xml)
        .ok()
        .and_then(|root| {
            root.get("child")?
                .get("children")?
                .as_array()?
                .iter()
                .find_map(|l| l.get("smooth").and_then(|s| s.as_bool()))
        })
        .unwrap_or(false)
}

/// Toggle the whole route between smooth (arcs through the waypoints) and straight
/// legs. See [`route_is_smooth`] for why the flag rides on the legs.
pub fn set_route_smooth(xml: &str, smooth: bool) -> Result<String, String> {
    let mut root = crate::btcpp_xml::xml_to_value(xml)?;
    let legs = legs_mut(&mut root)?;
    for leg in legs.iter_mut() {
        let Some(map) = leg.as_object_mut() else { continue };
        if map.get("kind").and_then(|k| k.as_str()) != Some("drive_to") {
            continue;
        }
        if smooth {
            map.insert("smooth".into(), Value::Bool(true));
        } else {
            map.remove("smooth");
        }
    }
    crate::btcpp_xml::value_to_xml(&root)
}

/// Remove every sequence leg whose `target` coord is in `reached` from `value`
/// **in memory**.
///
/// Called in [`compile_behavior_xml`] on the cloned JSON value before baking
/// targets to world coordinates. Keeps `xml.0` (the on-disk source) intact — every
/// leg stays for history/visualisation — while the compiled `BehaviorSpec` contains
/// only active (not-yet-reached) legs, so the rover advances. "Reached" is sourced
/// from the runtime [`ReachedWaypoints`] component, never from the document, so a
/// visited flag is live-only and resets on reload.
fn strip_reached_legs(v: &mut Value, reached: &std::collections::HashSet<String>) {
    if reached.is_empty() {
        return;
    }
    match v {
        Value::Object(map) => {
            if let Some(Value::Array(children)) = map.get_mut("children") {
                children.retain(|child| {
                    child
                        .get("target")
                        .and_then(|t| t.as_str())
                        .map(|t| !reached.contains(t))
                        .unwrap_or(true)
                });
            }
            for child in map.values_mut() {
                strip_reached_legs(child, reached);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| strip_reached_legs(i, reached)),
        _ => {}
    }
}

/// Spacing (world units) between resampled points on a `smooth` route. The rover
/// drives point-to-point, so this is the chord length of the arc it actually follows —
/// small enough to read as a curve, large enough not to bloat the tree.
const SMOOTH_SPACING: f64 = 2.0;
/// Arrival radius for a resampled point. Must be < [`SMOOTH_SPACING`] so the rover
/// keeps advancing along the curve instead of sitting inside two radii at once.
const SMOOTH_RADIUS: f64 = 1.5;

/// One Catmull-Rom segment: the curve passes exactly THROUGH `p1` (t=0) and `p2`
/// (t=1); `p0`/`p3` are the neighbouring control points that set the tangents.
fn catmull_rom(p0: DVec3, p1: DVec3, p2: DVec3, p3: DVec3, t: f64) -> DVec3 {
    let (t2, t3) = (t * t, t * t * t);
    (p1 * 2.0
        + (p2 - p0) * t
        + (p0 * 2.0 - p1 * 5.0 + p2 * 4.0 - p3) * t2
        + (p1 * 3.0 - p0 - p2 * 3.0 + p3) * t3)
        * 0.5
}

/// Sample a Catmull-Rom spline through `points` at roughly `spacing` intervals.
///
/// `closed` wraps the end back to the start (a `forever` patrol loop). The curve
/// passes through every control point, so the result is the same path the rover
/// drives — the route ribbon and the compiled legs share this function so what you
/// SEE is what it FOLLOWS. Fewer than 3 points can't form a curve, so they're
/// returned unchanged (a straight leg).
pub fn catmull_rom_path(points: &[DVec3], closed: bool, spacing: f64) -> Vec<DVec3> {
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    let spacing = spacing.max(0.25);
    let at = |i: i64| -> DVec3 {
        let n = n as i64;
        if closed {
            points[((i % n + n) % n) as usize]
        } else {
            points[i.clamp(0, n - 1) as usize]
        }
    };
    let seg_count = if closed { n } else { n - 1 };
    let mut out = Vec::new();
    for s in 0..seg_count as i64 {
        let (p0, p1, p2, p3) = (at(s - 1), at(s), at(s + 1), at(s + 2));
        let steps = (((p2 - p1).length().max(spacing)) / spacing).ceil().max(1.0) as usize;
        for k in 0..steps {
            out.push(catmull_rom(p0, p1, p2, p3, k as f64 / steps as f64));
        }
    }
    if !closed {
        out.push(points[n - 1]);
    }
    out
}

/// Parse a leg's `target="x;y;z"` into a point. `None` for a prim-path target.
fn parse_coord_target(s: &str) -> Option<DVec3> {
    let p: Vec<&str> = s.split(';').collect();
    if p.len() != 3 {
        return None;
    }
    match (p[0].trim().parse(), p[1].trim().parse(), p[2].trim().parse()) {
        (Ok(x), Ok(y), Ok(z)) => Some(DVec3::new(x, y, z)),
        _ => None,
    }
}

/// Expand the editor's `forever(sequence[drive_to…])` route **in memory**:
///
/// * per-leg `dwell="N"` → a `wait` leaf appended after that `drive_to`, so the rover
///   holds there before departing (`DriveTo` itself has no dwell field — serde would
///   silently ignore the attribute, so it must become a real node).
/// * route-level `smooth="true"` on the `Sequence` → the sparse control points are
///   resampled into dense `drive_to` legs along a Catmull-Rom curve, so the rover
///   ARCS through them (e.g. around an obstacle) instead of cutting hard corners.
///
/// Runtime-only: `xml.0` keeps the sparse control points and the flags. Anything that
/// isn't the plain editor patrol shape (a hand-authored tree, prim-path targets, mixed
/// node kinds) is left completely untouched for serde to handle as before.
fn expand_editor_route_in_place(v: &mut Value) {
    let closed = v.get("kind").and_then(|k| k.as_str()) == Some("forever");
    let Some(seq) = v
        .get_mut("child")
        .filter(|c| c.get("kind").and_then(|k| k.as_str()) == Some("sequence"))
    else {
        return;
    };
    let Some(children) = seq.get("children").and_then(|c| c.as_array()) else { return };

    // Only rewrite a route that is entirely coordinate `drive_to` legs — otherwise
    // leave the tree alone rather than restructure something hand-authored.
    let mut smooth = false;
    let mut ctrl: Vec<(DVec3, Option<f64>)> = Vec::with_capacity(children.len());
    for ch in children {
        if ch.get("kind").and_then(|k| k.as_str()) != Some("drive_to") {
            return;
        }
        let Some(pos) = ch.get("target").and_then(|t| t.as_str()).and_then(parse_coord_target)
        else {
            return;
        };
        // Route-level flag, carried per-leg (see `route_is_smooth`). Consumed here —
        // it is not a `BehaviorSpec` field.
        smooth |= ch.get("smooth").and_then(|s| s.as_bool()).unwrap_or(false);
        ctrl.push((pos, ch.get("dwell").and_then(|d| d.as_f64()).filter(|d| *d > 0.0)));
    }
    if ctrl.is_empty() {
        return;
    }

    let fmt = |p: DVec3| format!("{};{};{}", p.x, p.y, p.z);
    let drive = |target: String, radius: Option<f64>| {
        let mut m = serde_json::Map::new();
        m.insert("kind".into(), Value::String("drive_to".into()));
        m.insert("target".into(), Value::String(target));
        if let Some(r) = radius {
            m.insert("radius".into(), serde_json::json!(r));
        }
        Value::Object(m)
    };
    let wait = |secs: f64| serde_json::json!({ "kind": "wait", "seconds": secs });

    let mut out: Vec<Value> = Vec::new();
    if smooth && ctrl.len() >= 3 {
        // Generate the curve inline (rather than via `catmull_rom_path`) so each
        // control point's dwell lands on the sample that IS that control point —
        // Catmull-Rom hits p1 exactly at t=0, i.e. the first sample of its segment.
        let n = ctrl.len();
        let at = |i: i64| -> (DVec3, Option<f64>) {
            let n = n as i64;
            if closed { ctrl[((i % n + n) % n) as usize] } else { ctrl[i.clamp(0, n - 1) as usize] }
        };
        let seg_count = if closed { n } else { n - 1 };
        for s in 0..seg_count as i64 {
            let ((p0, _), (p1, d1), (p2, _), (p3, _)) = (at(s - 1), at(s), at(s + 1), at(s + 2));
            let steps = (((p2 - p1).length().max(SMOOTH_SPACING)) / SMOOTH_SPACING).ceil().max(1.0)
                as usize;
            for k in 0..steps {
                let t = k as f64 / steps as f64;
                out.push(drive(fmt(catmull_rom(p0, p1, p2, p3, t)), Some(SMOOTH_RADIUS)));
                if k == 0 {
                    if let Some(d) = d1 {
                        out.push(wait(d)); // dwell AT the control point
                    }
                }
            }
        }
        if !closed {
            let (last, dl) = ctrl[n - 1];
            out.push(drive(fmt(last), Some(SMOOTH_RADIUS)));
            if let Some(d) = dl {
                out.push(wait(d));
            }
        }
    } else {
        for (p, dwell) in &ctrl {
            out.push(drive(fmt(*p), None));
            if let Some(d) = dwell {
                out.push(wait(*d));
            }
        }
    }

    if let Some(map) = seq.as_object_mut() {
        map.insert("children".into(), Value::Array(out));
    }
}

/// Replace every prim-path `target` with the prim's live world position. Returns the
/// paths that could not be resolved — a tree naming a deleted waypoint must not
/// compile (it would drive to the origin).
///
/// Everything here is the BigSpace **root frame** — the frame `drive_autopilots`
/// ticks in, avian's `Position` uses, and an authored `x;y;z` waypoint already
/// means. This used to bake into the RENDER frame instead (prim positions read
/// from `GlobalTransform`, literal waypoints actively converted world→render via
/// the grid's floating offset). That frame is origin-relative, so every baked
/// coordinate silently expired whenever big_space moved the origin, and a JSON
/// `target: [x,y,z]` — which this never rewrites — was left in world coordinates
/// to be compared against render ones. In the sandbox the origin cell makes the
/// two frames equal, which is why only the moonbase broke.
fn bake_targets(
    v: &mut Value,
    bindings: &TargetBindings,
    pose: &dyn Fn(Entity) -> Option<DVec3>,
    missing: &mut Vec<String>,
) {
    match v {
        Value::Object(map) => {
            let resolved = match map.get("target") {
                Some(Value::String(s)) => {
                    if s.starts_with('/') {
                        match bindings.0.get(s.as_str()).and_then(|e| pose(*e)) {
                            Some(p) => Some(serde_json::json!([p.x, p.y, p.z])),
                            None => {
                                missing.push(s.clone());
                                None
                            }
                        }
                    } else {
                        let parts: Vec<&str> = s.split(';').collect();
                        if parts.len() == 3 {
                            if let (Ok(x), Ok(y), Ok(z)) = (
                                parts[0].trim().parse::<f64>(),
                                parts[1].trim().parse::<f64>(),
                                parts[2].trim().parse::<f64>(),
                            ) {
                                // Verbatim: an authored waypoint is ALREADY in the
                                // frame the tick runs in.
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
                bake_targets(child, bindings, pose, missing);
            }
        }
        Value::Array(items) =>
            items
                .iter_mut()
                .for_each(|i| bake_targets(i, bindings, pose, missing)),
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
    q_autopilots: Query<(Entity, &Autopilot, Has<AutopilotBehavior>)>,
    q_spec: Query<&AutopilotBehaviorSpec>,
    q_reached: Query<&ReachedWaypoints>,
    moved: Query<
        Entity,
        Or<(
            Changed<BehaviorXml>,
            Changed<TargetBindings>,
            Changed<GlobalTransform>,
            Changed<ReachedWaypoints>,
        )>,
    >,
    q_grids_only: Query<&big_space::prelude::Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&big_space::grid::cell::CellCoord>, &Transform)>,
    mut commands: Commands,
) {
    if q_vessels.is_empty() || moved.is_empty() {
        return;
    }
    // Root-frame position of any prim, straight off the cell chain — no grid
    // offset to subtract and no floating origin to chase, because the bake no
    // longer targets the render frame.
    let pose = |e: Entity| lunco_core::coords::world_position(e, &q_parents, &q_grids_only, &q_spatial);

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
        // Strip already-reached legs from the in-memory value BEFORE baking — the
        // compiled BehaviorSpec then only carries active legs so the rover advances,
        // while the on-disk xml.0 keeps every leg. "Reached" is RUNTIME-ONLY, read
        // from the live `ReachedWaypoints` component, never from the document.
        if let Ok(reached) = q_reached.get(vessel) {
            strip_reached_legs(&mut value, &reached.0);
        }
        // Expand the editor route: per-leg `dwell` → a `wait` leaf, and route-level
        // `smooth` → dense Catmull-Rom drive_to legs so the rover ARCS through the
        // control points instead of cutting corners. Runtime-only — xml.0 keeps the
        // sparse control points + the dwell/smooth flags. Done AFTER strip (reached
        // points drop out of the curve) and BEFORE bake (legs are still
        // `target="x;y;z"` strings that the resampler parses).
        expand_editor_route_in_place(&mut value);
        bake_targets(&mut value, bindings, &pose, &mut missing);
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

        // Only touch anything when the DERIVED spec actually differs from the one
        // already on the vessel. This system is gated on `Changed<GlobalTransform>`
        // over ALL entities, so it re-runs every frame the rover moves; rebuilding
        // the tree unconditionally would hand the actor a FRESH `AutopilotBehavior`
        // each tick and reset all behaviour state with it — the sequence would snap
        // back to leg 0 and, worse, every `WaitNode` timer would restart from zero, so
        // a waypoint `dwell` could never elapse. Re-baking is idempotent for a static
        // route, so an unchanged spec means there is nothing to do.
        let spec_changed = q_spec.get(vessel).map(|cur| cur.0 != spec).unwrap_or(true);
        if spec_changed {
            // The spec on the vessel is DERIVED — a projection of the XML + the prims.
            commands.entity(vessel).try_insert(AutopilotBehaviorSpec(spec.clone()));
        }
        // Live hot-swap: if an autopilot is already driving, re-point it at the edited
        // route without a disengage/re-engage cycle. Also covers an autopilot engaged
        // with no tree of its own (empty `spec_json`), which would otherwise never
        // pick up the vessel's authored route.
        if let Some((actor, _, has_tree)) = q_autopilots.iter().find(|(_, ap, _)| ap.vessel == vessel) {
            if spec_changed || !has_tree {
                commands.entity(actor).try_insert(AutopilotBehavior::new(&spec));
            }
        }
    }
}

#[cfg(test)]
mod bake_frame_tests {
    //! The bake's frame contract. Every coordinate a compiled tree carries is in
    //! the BigSpace ROOT frame — the frame `drive_autopilots` ticks in and avian's
    //! `Position` uses. The bake used to target the RENDER frame instead, which is
    //! origin-relative: every baked coordinate expired whenever big_space moved the
    //! origin, and an authored waypoint got actively converted into it. The sandbox
    //! sits in the origin cell, where the two frames are equal — so only the
    //! moonbase, whose origin is cells away, showed it.
    use super::*;

    fn drive_to(target: &str) -> Value {
        serde_json::json!({ "kind": "drive_to", "target": target })
    }

    /// An authored `x;y;z` waypoint is ALREADY root-frame; the bake must pass it
    /// through untouched rather than rebasing it onto the floating origin.
    #[test]
    fn a_literal_waypoint_bakes_verbatim() {
        let mut v = drive_to("1200.5;-53;-800");
        let mut missing = Vec::new();
        let pose = |_: Entity| -> Option<DVec3> { panic!("a literal target must not resolve a prim") };

        bake_targets(&mut v, &TargetBindings::default(), &pose, &mut missing);

        assert_eq!(v["target"], serde_json::json!([1200.5, -53.0, -800.0]));
        assert!(missing.is_empty());
    }

    /// A prim target resolves through the cell chain — the same root frame, so a
    /// pin two cells out bakes to its true position, not a render-frame shadow.
    #[test]
    fn a_prim_waypoint_bakes_its_root_frame_position() {
        let mut world = World::new();
        let pin = world.spawn_empty().id();
        let mut bindings = TargetBindings::default();
        bindings.0.insert("/Scene/Pin".to_string(), pin);

        let mut v = drive_to("/Scene/Pin");
        let mut missing = Vec::new();
        // Two cells up on a 2 km grid, 53 m down within the cell.
        let pose = |e: Entity| -> Option<DVec3> {
            (e == pin).then_some(DVec3::new(10.0, 3947.0, 4.0))
        };

        bake_targets(&mut v, &bindings, &pose, &mut missing);

        assert_eq!(v["target"], serde_json::json!([10.0, 3947.0, 4.0]));
        assert!(missing.is_empty());
    }

    /// A dangling prim reference is reported, never silently baked to the origin —
    /// that would drive the rover to grid-zero.
    #[test]
    fn an_unresolved_prim_target_is_reported() {
        let mut v = drive_to("/Scene/Deleted");
        let mut missing = Vec::new();
        let pose = |_: Entity| -> Option<DVec3> { None };

        bake_targets(&mut v, &TargetBindings::default(), &pose, &mut missing);

        assert_eq!(missing, vec!["/Scene/Deleted".to_string()]);
        // Left as the unresolved path, NOT rewritten to a coordinate.
        assert_eq!(v["target"], serde_json::json!("/Scene/Deleted"));
    }

    /// Nested legs bake too — the walk recurses through arrays and objects.
    #[test]
    fn nested_legs_bake() {
        let mut v = serde_json::json!({
            "kind": "sequence",
            "children": [drive_to("100;0;200"), drive_to("300;0;400")],
        });
        let mut missing = Vec::new();
        let pose = |_: Entity| -> Option<DVec3> { None };

        bake_targets(&mut v, &TargetBindings::default(), &pose, &mut missing);

        assert_eq!(v["children"][0]["target"], serde_json::json!([100.0, 0.0, 200.0]));
        assert_eq!(v["children"][1]["target"], serde_json::json!([300.0, 0.0, 400.0]));
    }
}

#[cfg(test)]
mod editor_tests {
    use super::*;

    /// The shape the waypoint editor authors: `forever(sequence[drive_to…])`.
    fn route(targets: &[&str]) -> String {
        let legs: String = targets
            .iter()
            .map(|t| format!("        <Action ID=\"drive_to\" target=\"{t}\"/>\n"))
            .collect();
        format!(
            "<root BTCPP_format=\"4\" main_tree_to_execute=\"MainTree\">\n  \
             <BehaviorTree ID=\"MainTree\">\n    <Repeat num_cycles=\"-1\">\n      \
             <Sequence>\n{legs}      </Sequence>\n    </Repeat>\n  </BehaviorTree>\n</root>"
        )
    }

    fn targets_of(xml: &str) -> Vec<String> {
        let v = crate::btcpp_xml::xml_to_value(xml).unwrap();
        v["child"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|l| l.get("target").and_then(|t| t.as_str()).map(String::from))
            .collect()
    }

    #[test]
    fn move_repoints_the_leg_in_place() {
        let xml = route(&["1;0;1", "2;0;2", "3;0;3"]);
        let out = set_waypoint_target(&xml, "2;0;2", "9;0;9").unwrap();
        // Repointed, and crucially still the SECOND leg — a move must not reorder.
        assert_eq!(targets_of(&out), vec!["1;0;1", "9;0;9", "3;0;3"]);
    }

    #[test]
    fn insert_after_lands_next_to_its_anchor_not_at_the_end() {
        let xml = route(&["1;0;1", "2;0;2", "3;0;3"]);
        let out = insert_waypoint_after(&xml, "1;0;1", "5;0;5").unwrap();
        assert_eq!(targets_of(&out), vec!["1;0;1", "5;0;5", "2;0;2", "3;0;3"]);
    }

    #[test]
    fn dwell_round_trips_and_zero_clears_it() {
        let xml = route(&["1;0;1", "2;0;2"]);
        let out = set_waypoint_dwell(&xml, "2;0;2", 3.5).unwrap();
        assert_eq!(waypoint_dwell(&out, "2;0;2"), Some(3.5));
        assert_eq!(waypoint_dwell(&out, "1;0;1"), None, "dwell must not leak to siblings");
        let cleared = set_waypoint_dwell(&out, "2;0;2", 0.0).unwrap();
        assert_eq!(waypoint_dwell(&cleared, "2;0;2"), None);
    }

    #[test]
    fn smooth_flag_round_trips_through_xml() {
        let xml = route(&["1;0;1", "2;0;2"]);
        assert!(!route_is_smooth(&xml));
        let on = set_route_smooth(&xml, true).unwrap();
        assert!(route_is_smooth(&on), "smooth must survive the XML round-trip");
        let off = set_route_smooth(&on, false).unwrap();
        assert!(!route_is_smooth(&off));
    }

    #[test]
    fn dwell_expands_to_a_real_wait_node() {
        // `DriveTo` has no dwell field, so the attribute alone would be silently
        // ignored by serde — it MUST become a `wait` leaf for the rover to pause.
        let xml = set_waypoint_dwell(&route(&["1;0;1", "2;0;2"]), "1;0;1", 2.0).unwrap();
        let mut v = crate::btcpp_xml::xml_to_value(&xml).unwrap();
        expand_editor_route_in_place(&mut v);
        let kinds: Vec<&str> = v["child"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, vec!["drive_to", "wait", "drive_to"]);
        assert_eq!(v["child"]["children"][1]["seconds"].as_f64(), Some(2.0));
    }

    #[test]
    fn smooth_resamples_into_dense_legs_through_the_control_points() {
        // Three far-apart points → the curve must densify well beyond 3 legs, and
        // still pass exactly through each authored control point.
        let xml = set_route_smooth(&route(&["0;0;0", "20;0;0", "20;0;20"]), true).unwrap();
        let mut v = crate::btcpp_xml::xml_to_value(&xml).unwrap();
        expand_editor_route_in_place(&mut v);
        let legs = v["child"]["children"].as_array().unwrap().clone();
        assert!(legs.len() > 10, "expected a densified curve, got {} legs", legs.len());
        let targets: Vec<String> =
            legs.iter().filter_map(|l| l["target"].as_str().map(String::from)).collect();
        for ctrl in ["0;0;0", "20;0;0", "20;0;20"] {
            let p = parse_coord_target(ctrl).unwrap();
            assert!(
                targets.iter().filter_map(|t| parse_coord_target(t)).any(|q| (q - p).length() < 1e-6),
                "curve must pass through control point {ctrl}"
            );
        }
        // `smooth` is consumed by the expansion — it is not a BehaviorSpec field, so
        // no resampled leg may carry it through to serde.
        assert!(legs.iter().all(|l| l.get("smooth").is_none()));
    }

    #[test]
    fn a_straight_route_is_left_as_authored() {
        let mut v = crate::btcpp_xml::xml_to_value(&route(&["1;0;1", "2;0;2"])).unwrap();
        expand_editor_route_in_place(&mut v);
        assert_eq!(targets_of(&crate::btcpp_xml::value_to_xml(&v).unwrap()), vec!["1;0;1", "2;0;2"]);
    }

    #[test]
    fn catmull_rom_passes_through_every_control_point() {
        let pts = vec![DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0), DVec3::new(10.0, 0.0, 10.0)];
        let path = catmull_rom_path(&pts, false, 2.0);
        assert!(path.len() > pts.len());
        for p in &pts {
            assert!(path.iter().any(|q| (*q - *p).length() < 1e-6), "missing control point {p:?}");
        }
    }
}
