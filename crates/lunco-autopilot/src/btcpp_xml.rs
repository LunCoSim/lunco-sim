//! BehaviorTree.CPP v4 XML ⇄ tree-JSON codec.
//!
//! A behaviour tree in this project is authored as DATA — a `BehaviorSpec` JSON
//! (internally tagged by `kind`; see [`crate::BehaviorSpec`]). This module
//! translates that JSON to and from **BehaviorTree.CPP v4 XML**, the de-facto
//! robotics interchange format (Groot2 editor, ROS/Nav2). Round-tripping a tree
//! through XML lets the same behaviour be edited in Groot2 or run by real flight
//! software, then brought back.
//!
//! ## One source of truth
//!
//! `TABLE` is the whole mapping: one row per `BehaviorSpec` kind, giving its XML
//! element and its structural shape (control / decorator / leaf). `spec_kind` is an
//! **exhaustive match over the enum**, so adding a variant to `BehaviorSpec` fails to
//! compile until it is given a row — the table cannot drift from the enum.
//!
//! ## Fidelity rules
//!
//! * **Attribute values are JSON.** Numbers, bools, arrays and objects are written as
//!   JSON text (`waypoints="[[10.0,0.0,-5.0]]"`) and decoded by *shape*, so a
//!   one-element vector-of-vectors and an empty vector survive. A string that would
//!   itself parse as JSON is written quoted (`label="&quot;42&quot;"`) so it comes
//!   back a string; anything else (`{goal}` blackboard refs, plain words) is written
//!   verbatim. Newlines/tabs are numeric character references, because XML
//!   attribute-value normalisation would otherwise turn them into spaces.
//! * **Canonical numbers.** `times`/`num_cycles` are whole, `seconds` fractional. A
//!   hand-authored `"times": 3.0` normalises to `3` on the way back.
//! * **Foreign elements keep their subtree.** Any element with no row in `TABLE`
//!   (`<Delay>`, `<IfThenElse>`, `<KeepRunningUntilFailure>`, …) becomes
//!   `{"kind": snake_case_tag, "children": [...]}` and is written back out as its
//!   original element — nothing is dropped. A custom `<Action ID="x"/>` keeps its
//!   `<Action>` form (a foreign leaf carries no `children` key).
//! * **`<SubTree>` is resolved, not represented.** `BehaviorSpec` has no sub-tree
//!   variant, so an imported `<SubTree ID="X"/>` is expanded in place from the file's
//!   other `<BehaviorTree>`s (`main_tree_to_execute` selects the entry point);
//!   recursion and dangling references are errors.
//! * **Depth is capped** at [`MAX_DEPTH`] in both directions — the JSON produced by an
//!   import is `serde_json::to_string`'d by the caller, which recurses.

use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::name::QName;
use quick_xml::{Reader, Writer};
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Cursor;

/// Maximum nesting accepted in either direction. Below serde_json's 128-deep
/// recursion limit, so an adversarial XML file errors instead of overflowing the
/// stack when the imported `Value` is serialised.
pub const MAX_DEPTH: usize = 64;

/// The `<BehaviorTree ID>` an export writes (and `main_tree_to_execute` names).
const MAIN_TREE: &str = "MainTree";

/// JSON keys that carry structure, never a value — they may not appear as XML
/// attributes (an `<Action ID="wait" kind="pwn"/>` must not be able to rewrite the
/// node's kind).
const RESERVED: &[&str] = &["kind", "child", "children"];

// ── The mapping table (single source of truth) ───────────────────────────────

/// How a kind is written as an XML element.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Elem {
    /// A dedicated BT.CPP element, e.g. `<Sequence>`.
    Tag(&'static str),
    /// `<Action ID="kind" …/>` — a leaf with no standard element.
    Action,
    /// `<Condition ID="kind" …/>` — a pure-condition leaf.
    Condition,
}

/// The structural slot a kind's subtree lives in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// `children: [...]`.
    Control,
    /// `child: {...}` — exactly one.
    Decorator,
    /// No subtree.
    Leaf,
}

/// One row: a `BehaviorSpec` kind ⇄ its BT.CPP element.
struct KindMap {
    kind: &'static str,
    elem: Elem,
    shape: Shape,
}

const fn ctrl(kind: &'static str, tag: &'static str) -> KindMap {
    KindMap {
        kind,
        elem: Elem::Tag(tag),
        shape: Shape::Control,
    }
}
const fn deco(kind: &'static str, tag: &'static str) -> KindMap {
    KindMap {
        kind,
        elem: Elem::Tag(tag),
        shape: Shape::Decorator,
    }
}
const fn leaf(kind: &'static str, tag: &'static str) -> KindMap {
    KindMap {
        kind,
        elem: Elem::Tag(tag),
        shape: Shape::Leaf,
    }
}
const fn action(kind: &'static str) -> KindMap {
    KindMap {
        kind,
        elem: Elem::Action,
        shape: Shape::Leaf,
    }
}
const fn cond(kind: &'static str) -> KindMap {
    KindMap {
        kind,
        elem: Elem::Condition,
        shape: Shape::Leaf,
    }
}

/// Every `BehaviorSpec` kind, exactly once. `repeat` precedes `forever` because both
/// are `<Repeat>` — a negative `num_cycles` (BT.CPP's "loop forever") decodes to
/// `forever`, everything else to `repeat`.
static TABLE: &[KindMap] = &[
    // Controls
    ctrl("sequence", "Sequence"),
    ctrl("selector", "Fallback"),
    ctrl("reactive_sequence", "ReactiveSequence"),
    ctrl("reactive_selector", "ReactiveFallback"),
    ctrl("parallel", "Parallel"),
    // Decorators
    deco("repeat", "Repeat"),
    deco("forever", "Repeat"),
    deco("retry", "RetryUntilSuccessful"),
    deco("invert", "Inverter"),
    deco("force_success", "ForceSuccess"),
    deco("force_failure", "ForceFailure"),
    deco("timeout", "Timeout"),
    // Cooldown has no BT.CPP-standard element; emit a custom decorator the reverse map
    // knows, so it still round-trips (Groot shows it as a custom node).
    deco("cooldown", "Cooldown"),
    // Leaves with a standard element
    leaf("succeed", "AlwaysSuccess"),
    leaf("fail", "AlwaysFailure"),
    // Action leaves
    action("drive_to"),
    action("patrol"),
    action("wait"),
    action("cruise"),
    action("brake"),
    action("face"),
    action("hold"),
    action("follow"),
    action("intercept"),
    action("steer_clear"),
    // Fires a named tool once per activation (e.g. `science::take_photo`). Round-trips as
    // `<Action ID="run_tool" tool="…" args="…"/>` — see `usd_tree.rs`.
    action("run_tool"),
    // Condition leaves
    cond("arrived"),
    cond("facing"),
    cond("obstacle_ahead"),
    cond("path_blocked"),
];

/// Compile-time guard: the exhaustive match means a new `BehaviorSpec` variant does
/// not build until it names a kind, and `table_covers_every_spec_kind` proves that
/// kind has a [`TABLE`] row.
#[allow(dead_code)]
fn spec_kind(spec: &crate::BehaviorSpec) -> &'static str {
    use crate::BehaviorSpec as B;
    match spec {
        B::Sequence { .. } => "sequence",
        B::Selector { .. } => "selector",
        B::Parallel { .. } => "parallel",
        B::ReactiveSequence { .. } => "reactive_sequence",
        B::ReactiveSelector { .. } => "reactive_selector",
        B::Forever { .. } => "forever",
        B::Repeat { .. } => "repeat",
        B::Retry { .. } => "retry",
        B::Invert { .. } => "invert",
        B::ForceSuccess { .. } => "force_success",
        B::ForceFailure { .. } => "force_failure",
        B::Timeout { .. } => "timeout",
        B::Cooldown { .. } => "cooldown",
        B::DriveTo { .. } => "drive_to",
        B::Patrol { .. } => "patrol",
        B::Arrived { .. } => "arrived",
        B::Wait { .. } => "wait",
        B::Cruise { .. } => "cruise",
        B::Brake => "brake",
        B::Face { .. } => "face",
        B::Facing { .. } => "facing",
        B::Succeed => "succeed",
        B::Fail => "fail",
        B::Follow { .. } => "follow",
        B::Intercept { .. } => "intercept",
        B::ObstacleAhead { .. } => "obstacle_ahead",
        B::PathBlocked { .. } => "path_blocked",
        B::Hold => "hold",
        B::SteerClear { .. } => "steer_clear",
        B::RunTool { .. } => "run_tool",
    }
}

fn by_kind(kind: &str) -> Option<&'static KindMap> {
    TABLE.iter().find(|k| k.kind == kind)
}

fn by_tag(tag: &str) -> Option<&'static KindMap> {
    TABLE
        .iter()
        .find(|k| matches!(k.elem, Elem::Tag(t) if t == tag))
}

// ── JSON → XML ───────────────────────────────────────────────────────────────

/// Serialise a tree-JSON node into a full BehaviorTree.CPP v4 XML document.
pub fn value_to_xml(root: &Value) -> Result<String, String> {
    let mut w = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);
    let mut r = BytesStart::new("root");
    push_attr(&mut r, "BTCPP_format", "4");
    push_attr(&mut r, "main_tree_to_execute", MAIN_TREE);
    w.write_event(Event::Start(r)).map_err(se)?;
    let mut bt = BytesStart::new("BehaviorTree");
    push_attr(&mut bt, "ID", MAIN_TREE);
    w.write_event(Event::Start(bt)).map_err(se)?;
    write_node(&mut w, root, 0)?;
    w.write_event(Event::End(BytesEnd::new("BehaviorTree")))
        .map_err(se)?;
    w.write_event(Event::End(BytesEnd::new("root")))
        .map_err(se)?;
    String::from_utf8(w.into_inner().into_inner()).map_err(|e| e.to_string())
}

type Xw = Writer<Cursor<Vec<u8>>>;

fn write_node(w: &mut Xw, node: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "behaviour tree is nested deeper than {MAX_DEPTH} nodes"
        ));
    }
    let obj = node.as_object().ok_or("tree node is not an object")?;
    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("tree node is missing a `kind`")?;
    match by_kind(kind) {
        Some(km) => write_known(w, km, obj, depth),
        None => write_foreign(w, kind, obj, depth),
    }
}

/// Write a node whose kind has a [`TABLE`] row.
fn write_known(
    w: &mut Xw,
    km: &KindMap,
    obj: &Map<String, Value>,
    depth: usize,
) -> Result<(), String> {
    let kind = km.kind;
    // Spec fields that are renamed on the wire. `consumed` keys are not re-emitted raw.
    let (extra, consumed): (Vec<(&str, String)>, &[&str]) = match kind {
        "parallel" => {
            // BT.CPP v4 has no `require` — it resolves a Parallel by success_count.
            let req = obj.get("require").and_then(Value::as_str).unwrap_or("all");
            let count = match req {
                "all" => "-1",
                "one" => "1",
                other => return Err(format!("`parallel`: unknown require `{other}`")),
            };
            (vec![("success_count", count.into())], &["require"])
        }
        "forever" => (vec![("num_cycles", "-1".into())], &[]),
        "repeat" => (
            vec![("num_cycles", whole_field(obj, "times", kind)?)],
            &["times"],
        ),
        "retry" => (
            vec![("num_attempts", whole_field(obj, "times", kind)?)],
            &["times"],
        ),
        "timeout" => (
            vec![("msec", num_text(secs_field(obj, kind)? * 1000.0))],
            &["seconds"],
        ),
        "cooldown" => (
            vec![("sec", num_text(secs_field(obj, kind)?))],
            &["seconds"],
        ),
        _ => (Vec::new(), &[]),
    };

    let tag = match km.elem {
        Elem::Tag(t) => t,
        Elem::Action => "Action",
        Elem::Condition => "Condition",
    };
    let mut e = BytesStart::new(tag);
    if matches!(km.elem, Elem::Action | Elem::Condition) {
        push_attr(&mut e, "ID", kind);
    }
    for (k, v) in &extra {
        push_attr(&mut e, k, v);
    }
    // Remaining scalar fields ride as attributes (leaf ports, and any field a renamed
    // kind did not consume).
    let attrs: Vec<(&str, String)> = obj
        .iter()
        .filter(|(k, _)| !RESERVED.contains(&k.as_str()) && !consumed.contains(&k.as_str()))
        .map(|(k, v)| (k.as_str(), attr_from_value(v)))
        .collect();
    for (k, v) in &attrs {
        push_attr(&mut e, k, v);
    }

    match km.shape {
        Shape::Leaf => {
            if obj.contains_key("child") || obj.contains_key("children") {
                return Err(format!("`{kind}` is a leaf and cannot carry a subtree"));
            }
            w.write_event(Event::Empty(e)).map_err(se)
        }
        Shape::Control => {
            let children = obj
                .get("children")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("`{kind}` is a control node and needs `children`"))?;
            w.write_event(Event::Start(e)).map_err(se)?;
            for c in children {
                write_node(w, c, depth + 1)?;
            }
            w.write_event(Event::End(BytesEnd::new(tag))).map_err(se)
        }
        Shape::Decorator => {
            let child = obj
                .get("child")
                .filter(|c| !c.is_null())
                .ok_or_else(|| format!("`{kind}` is a decorator and needs a `child`"))?;
            w.write_event(Event::Start(e)).map_err(se)?;
            write_node(w, child, depth + 1)?;
            w.write_event(Event::End(BytesEnd::new(tag))).map_err(se)
        }
    }
}

/// Write a node whose kind has no [`TABLE`] row — i.e. one imported from a BT.CPP file
/// that uses an element this crate has no spec for. A `children` key means it came in
/// as an element (`<Delay>`), so it goes back out as that element; without one it came
/// in as a custom `<Action ID="…"/>`.
fn write_foreign(
    w: &mut Xw,
    kind: &str,
    obj: &Map<String, Value>,
    depth: usize,
) -> Result<(), String> {
    let attrs: Vec<(&str, String)> = obj
        .iter()
        .filter(|(k, _)| !RESERVED.contains(&k.as_str()))
        .map(|(k, v)| (k.as_str(), attr_from_value(v)))
        .collect();

    let Some(children) = obj.get("children") else {
        if obj.contains_key("child") {
            return Err(format!(
                "`{kind}` is not a known kind; a foreign node holds its subtree in `children`, not `child`"
            ));
        }
        let mut e = BytesStart::new("Action");
        push_attr(&mut e, "ID", kind);
        for (k, v) in &attrs {
            push_attr(&mut e, k, v);
        }
        return w.write_event(Event::Empty(e)).map_err(se);
    };

    let children = children
        .as_array()
        .ok_or_else(|| format!("`{kind}`: `children` is not an array"))?;
    let tag = camel(kind);
    let mut e = BytesStart::new(tag.clone());
    for (k, v) in &attrs {
        push_attr(&mut e, k, v);
    }
    w.write_event(Event::Start(e)).map_err(se)?;
    for c in children {
        write_node(w, c, depth + 1)?;
    }
    w.write_event(Event::End(BytesEnd::new(tag))).map_err(se)
}

/// Render a JSON value as an attribute string. Arrays/objects/numbers/bools are JSON
/// text; a string that would itself parse as JSON is quoted so it decodes back to a
/// string.
fn attr_from_value(v: &Value) -> String {
    match v {
        Value::String(s) if serde_json::from_str::<Value>(s).is_ok() => {
            Value::String(s.clone()).to_string()
        }
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A whole, non-negative count (`times`), tolerating a JSON float (`3.0`).
fn whole_field(obj: &Map<String, Value>, key: &str, kind: &str) -> Result<String, String> {
    let n = obj
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("`{kind}` needs a numeric `{key}`"))?;
    if !n.is_finite() || n < 0.0 || n.fract() != 0.0 {
        return Err(format!(
            "`{kind}`: `{key}` must be a whole, non-negative number (got {n})"
        ));
    }
    Ok((n as i64).to_string())
}

/// `seconds`, defaulting to the spec's 1.0 when absent.
fn secs_field(obj: &Map<String, Value>, kind: &str) -> Result<f64, String> {
    match obj.get("seconds") {
        None => Ok(1.0),
        Some(v) => v
            .as_f64()
            .filter(|f| f.is_finite())
            .ok_or_else(|| format!("`{kind}`: `seconds` must be a finite number")),
    }
}

/// Shortest lossless decimal: whole values as integers (BT.CPP expects `msec="500"`),
/// fractional ones in full (so a 0.4 ms timeout is not rounded to zero).
fn num_text(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 9e15 {
        (v as i64).to_string()
    } else {
        v.to_string()
    }
}

/// Push an attribute, escaping the value ourselves: quick-xml escapes `< > & ' "` but
/// not `\n`/`\t`/`\r`, which XML attribute-value normalisation would flatten to spaces.
fn push_attr<'a>(e: &mut BytesStart<'a>, key: &'a str, value: &str) {
    e.push_attribute(Attribute {
        key: QName(key.as_bytes()),
        value: Cow::Owned(xml_escape(value).into_bytes()),
    });
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            _ => out.push(c),
        }
    }
    out
}

fn se<E: std::fmt::Display>(e: E) -> String {
    format!("xml write error: {e}")
}

// ── XML → JSON ───────────────────────────────────────────────────────────────

/// Internal marker for an unresolved `<SubTree ID="…"/>`; never survives
/// [`xml_to_value`] (it is expanded, or the import errors).
const SUBTREE: &str = "__subtree";

/// A partially-built node while descending the pull-parser event stream.
struct Frame {
    tag: String,
    attrs: Vec<(String, String)>,
    children: Vec<Value>,
}

/// Parse a BehaviorTree.CPP v4 XML document into a tree-JSON node. `<root>` and
/// `<BehaviorTree>` are containers: the entry tree is the one `main_tree_to_execute`
/// names (or the only one), and any `<SubTree>` it references is expanded in place.
pub fn xml_to_value(xml: &str) -> Result<Value, String> {
    // We only act on Start/Empty/End/Eof; whitespace Text events between elements fall
    // through the `_ => {}` arm, so no text-trimming config is needed.
    let mut reader = Reader::from_str(xml);

    let mut stack: Vec<Frame> = Vec::new();
    // (BehaviorTree ID, its single root node), in document order.
    let mut trees: Vec<(String, Value)> = Vec::new();
    let mut main: Option<String> = None;

    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(e) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(format!("XML is nested deeper than {MAX_DEPTH} elements"));
                }
                stack.push(Frame {
                    tag: tag_name(e.name().as_ref()),
                    attrs: read_attrs(&e)?,
                    children: Vec::new(),
                });
            }
            Event::Empty(e) => {
                let f = Frame {
                    tag: tag_name(e.name().as_ref()),
                    attrs: read_attrs(&e)?,
                    children: Vec::new(),
                };
                close(f, &mut stack, &mut trees, &mut main)?;
            }
            Event::End(_) => {
                let f = stack.pop().ok_or("unbalanced XML")?;
                close(f, &mut stack, &mut trees, &mut main)?;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err("unbalanced XML".into());
    }
    if trees.is_empty() {
        return Err("no behaviour-tree node found in XML".into());
    }

    let by_id: HashMap<&str, &Value> = trees.iter().map(|(id, v)| (id.as_str(), v)).collect();
    let entry = match &main {
        Some(id) => *by_id
            .get(id.as_str())
            .ok_or_else(|| format!("main_tree_to_execute=\"{id}\" names no <BehaviorTree>"))?,
        None if trees.len() == 1 => &trees[0].1,
        None => {
            return Err(
                "several <BehaviorTree> elements but no main_tree_to_execute on <root>".into(),
            )
        }
    };
    expand(entry, &by_id, &mut Vec::new(), 0)
}

/// A finished element: `<root>`/`<BehaviorTree>` are containers, everything else is a
/// node that attaches to its parent (or, unwrapped, is a tree in its own right).
fn close(
    f: Frame,
    stack: &mut Vec<Frame>,
    trees: &mut Vec<(String, Value)>,
    main: &mut Option<String>,
) -> Result<(), String> {
    let get = |k: &str| f.attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
    match f.tag.as_str() {
        "root" => {
            *main = get("main_tree_to_execute");
            // A node placed straight under <root>, with no <BehaviorTree>, is a tree too.
            for c in f.children {
                trees.push((String::new(), c));
            }
        }
        "BehaviorTree" => {
            let id = get("ID").unwrap_or_default();
            let mut kids = f.children;
            if kids.len() != 1 {
                return Err(format!(
                    "<BehaviorTree ID=\"{id}\"> must hold exactly one node, found {}",
                    kids.len()
                ));
            }
            trees.push((id, kids.remove(0)));
        }
        _ => {
            let node = frame_to_value(&f.tag, &f.attrs, f.children)?;
            match stack.last_mut() {
                Some(parent) => parent.children.push(node),
                None => trees.push((String::new(), node)),
            }
        }
    }
    Ok(())
}

fn frame_to_value(
    tag: &str,
    attrs: &[(String, String)],
    children: Vec<Value>,
) -> Result<Value, String> {
    let get = |k: &str| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
    let mut m = Map::new();

    if tag == "SubTree" {
        let id = get("ID").ok_or("<SubTree> is missing its ID")?;
        m.insert(SUBTREE.into(), id.into());
        return Ok(Value::Object(m));
    }

    // A leaf named by ID: every known leaf, and any custom BT.CPP action/condition.
    if tag == "Action" || tag == "Condition" {
        let kind = get("ID").ok_or_else(|| format!("<{tag}> is missing its ID"))?;
        if !children.is_empty() {
            return Err(format!(
                "<{tag} ID=\"{kind}\"> is a leaf but holds child elements"
            ));
        }
        m.insert("kind".into(), kind.into());
        put_attrs(&mut m, attrs, tag)?;
        return Ok(Value::Object(m));
    }

    let Some(km) = by_tag(tag) else {
        // Foreign element (<Delay>, <IfThenElse>, …): keep the element, its ports AND
        // its whole subtree.
        m.insert("kind".into(), snake(tag).into());
        put_attrs(&mut m, attrs, tag)?;
        m.insert("children".into(), Value::Array(children));
        return Ok(Value::Object(m));
    };

    let mut kind = km.kind;
    match km.kind {
        "parallel" => {
            let count: i64 = get("success_count")
                .and_then(|s| s.parse().ok())
                .unwrap_or(-1);
            m.insert(
                "require".into(),
                if count == 1 { "one" } else { "all" }.into(),
            );
        }
        "repeat" => {
            let cycles: i64 = get("num_cycles").and_then(|s| s.parse().ok()).unwrap_or(1);
            if cycles < 0 {
                kind = "forever"; // BT.CPP spells "loop forever" as num_cycles="-1".
            } else {
                m.insert("times".into(), Value::from(cycles));
            }
        }
        "retry" => {
            let n: i64 = get("num_attempts")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1);
            m.insert("times".into(), Value::from(n.max(0)));
        }
        "timeout" => {
            let secs = get("msec").and_then(msec_to_secs).unwrap_or(1.0);
            m.insert("seconds".into(), Value::from(secs));
        }
        "cooldown" => {
            let s: f64 = get("sec").and_then(|s| s.parse().ok()).unwrap_or(1.0);
            m.insert("seconds".into(), Value::from(s));
        }
        // <AlwaysSuccess>/<AlwaysFailure> and the plain controls/decorators carry no
        // spec fields; Groot decorations (`name`, `_uid`) are dropped on purpose.
        _ => {}
    }
    m.insert("kind".into(), kind.into());

    let mut children = children;
    match km.shape {
        Shape::Control => {
            m.insert("children".into(), Value::Array(children));
        }
        Shape::Decorator => {
            if children.len() != 1 {
                return Err(format!(
                    "<{tag}> is a decorator and needs exactly one child, found {}",
                    children.len()
                ));
            }
            m.insert("child".into(), children.remove(0));
        }
        Shape::Leaf => {
            if !children.is_empty() {
                return Err(format!("<{tag}> is a leaf but holds child elements"));
            }
        }
    }
    Ok(Value::Object(m))
}

/// Copy an element's attributes into the node, rejecting the structural names.
fn put_attrs(
    m: &mut Map<String, Value>,
    attrs: &[(String, String)],
    tag: &str,
) -> Result<(), String> {
    for (k, v) in attrs {
        if k == "ID" {
            continue;
        }
        if RESERVED.contains(&k.as_str()) {
            return Err(format!("<{tag}> has a reserved attribute `{k}`"));
        }
        m.insert(k.clone(), value_from_attr(v));
    }
    Ok(())
}

/// Milliseconds text → seconds. Scaled in the DECIMAL domain (`"0.4"` → `"0.4e-3"`),
/// not by dividing the parsed f64 — a multiply-then-divide double-rounds, and
/// `seconds: 0.0004` would not come back bit-identical.
fn msec_to_secs(text: &str) -> Option<f64> {
    format!("{text}e-3")
        .parse::<f64>()
        .ok()
        .or_else(|| text.parse::<f64>().ok().map(|ms| ms / 1000.0))
        .filter(|s| s.is_finite())
}

/// Recover a JSON value from an attribute string: JSON if it parses as JSON (numbers,
/// bools, `[[10,0,-5]]`, `[]`, quoted strings), else the text itself (`{goal}`
/// blackboard refs, `NaN`, plain words).
fn value_from_attr(s: &str) -> Value {
    serde_json::from_str::<Value>(s).unwrap_or_else(|_| Value::String(s.to_string()))
}

/// Substitute every `<SubTree ID="X"/>` with tree `X`, depth- and cycle-checked.
fn expand(
    node: &Value,
    trees: &HashMap<&str, &Value>,
    visiting: &mut Vec<String>,
    depth: usize,
) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "behaviour tree is nested deeper than {MAX_DEPTH} nodes"
        ));
    }
    let obj = node.as_object().ok_or("tree node is not an object")?;

    if let Some(id) = obj.get(SUBTREE).and_then(Value::as_str) {
        if visiting.iter().any(|v| v == id) {
            return Err(format!("<SubTree ID=\"{id}\"> is recursive"));
        }
        let tree = *trees
            .get(id)
            .ok_or_else(|| format!("<SubTree ID=\"{id}\"> names no <BehaviorTree>"))?;
        visiting.push(id.to_string());
        let out = expand(tree, trees, visiting, depth + 1)?;
        visiting.pop();
        return Ok(out);
    }

    let mut m = obj.clone();
    if let Some(kids) = obj.get("children").and_then(Value::as_array) {
        let kids: Result<Vec<Value>, String> = kids
            .iter()
            .map(|c| expand(c, trees, visiting, depth + 1))
            .collect();
        m.insert("children".into(), Value::Array(kids?));
    }
    if let Some(child) = obj.get("child") {
        m.insert("child".into(), expand(child, trees, visiting, depth + 1)?);
    }
    Ok(Value::Object(m))
}

fn tag_name(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn read_attrs(e: &BytesStart) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|x| x.to_string())?;
        let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        // `Implicit1_0`: BehaviorTree.CPP `.btcpp` files carry no XML declaration,
        // and the spec says an entity without one is XML 1.0 — the same rule the
        // superseded `unescape_value` applied unconditionally, so escaping
        // behaviour is unchanged.
        let v = a
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|x| x.to_string())?
            .into_owned();
        out.push((k, v));
    }
    Ok(out)
}

/// `IfThenElse` → `if_then_else`.
fn snake(tag: &str) -> String {
    let mut out = String::new();
    for (i, c) in tag.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// `if_then_else` → `IfThenElse` (the inverse of [`snake`] for foreign elements).
fn camel(kind: &str) -> String {
    kind.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BehaviorSpec as B;

    /// The table is exhaustive over `BehaviorSpec`: `spec_kind`'s match is compiler-
    /// checked, and every kind it can return has exactly one row here.
    #[test]
    fn table_covers_every_spec_kind() {
        let child = || Box::new(B::Brake);
        let every: Vec<B> = vec![
            B::Sequence { children: vec![] },
            B::Selector { children: vec![] },
            B::Parallel {
                require: crate::ParallelRequire::All,
                children: vec![],
            },
            B::ReactiveSequence { children: vec![] },
            B::ReactiveSelector { children: vec![] },
            B::Forever { child: child() },
            B::Repeat {
                times: 1,
                child: child(),
            },
            B::Retry {
                times: 1,
                child: child(),
            },
            B::Invert { child: child() },
            B::ForceSuccess { child: child() },
            B::ForceFailure { child: child() },
            B::Timeout {
                seconds: 1.0,
                child: child(),
            },
            B::Cooldown {
                seconds: 1.0,
                child: child(),
            },
            B::DriveTo {
                target: [0.0; 3],
                speed: 0.6,
                radius: 2.0,
            },
            B::Patrol {
                waypoints: vec![],
                speed: 0.6,
                radius: 2.0,
                dwell: 0.0,
            },
            B::Arrived {
                target: [0.0; 3],
                radius: 2.0,
            },
            B::Wait { seconds: 1.0 },
            B::Cruise {
                throttle: 0.0,
                steer: 0.0,
            },
            B::Brake,
            B::Face {
                target: [0.0; 3],
                tolerance: 8.0,
            },
            B::Facing {
                target: [0.0; 3],
                tolerance: 8.0,
            },
            B::Succeed,
            B::Fail,
            B::Follow {
                target: 0,
                speed: 0.6,
                radius: 5.0,
            },
            B::Intercept {
                target: 0,
                speed: 0.6,
                radius: 2.0,
                lead: 1.0,
            },
            B::ObstacleAhead {
                distance: 6.0,
                cone: 40.0,
            },
            B::PathBlocked { distance: 6.0 },
            B::Hold,
            B::SteerClear { speed: 0.6 },
            B::RunTool {
                tool: "science::take_photo".into(),
                args: String::new(),
            },
        ];
        for spec in &every {
            let kind = spec_kind(spec);
            assert!(by_kind(kind).is_some(), "`{kind}` has no TABLE row");
        }
        assert_eq!(
            every.len(),
            TABLE.len(),
            "TABLE has a row that no BehaviorSpec variant maps to (or vice versa)"
        );
    }

    #[test]
    fn table_kinds_are_unique() {
        let mut kinds: Vec<&str> = TABLE.iter().map(|k| k.kind).collect();
        kinds.sort_unstable();
        let n = kinds.len();
        kinds.dedup();
        assert_eq!(n, kinds.len(), "duplicate kind in TABLE");
    }

    #[test]
    fn snake_camel_are_inverses() {
        for tag in ["Delay", "IfThenElse", "KeepRunningUntilFailure", "Switch2"] {
            assert_eq!(camel(&snake(tag)), tag);
        }
    }
}
