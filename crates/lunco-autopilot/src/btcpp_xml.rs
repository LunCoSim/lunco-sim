//! BehaviorTree.CPP v4 XML ⇄ tree-JSON codec.
//!
//! A behaviour tree in this project is authored as DATA — a `BehaviorSpec` JSON
//! (internally tagged by `kind`; see [`crate::BehaviorSpec`]). This module
//! translates that JSON to and from **BehaviorTree.CPP v4 XML**, the de-facto
//! robotics interchange format (Groot2 editor, ROS/Nav2). Round-tripping a tree
//! through XML lets the same behaviour be edited in Groot2 or run by real flight
//! software, then brought back.
//!
//! The codec is deliberately generic over `serde_json::Value`, not the
//! `BehaviorSpec` enum: it keys only on the `kind` string and the structural
//! `children`/`child` fields, so every current node kind — and any added later —
//! round-trips without touching this file. Control and decorator kinds map to the
//! standard BT.CPP elements (`<Sequence>`, `<Fallback>`, `<Repeat>`, …); every
//! other kind is a leaf, emitted as `<Action ID="kind" …/>` (or `<Condition …>`),
//! with each scalar field as an XML attribute. Vectors serialise as `;`-joined
//! (`[10,0,0]` → `"10;0;0"`) and vector-of-vectors as `|`-joined groups.

use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::{Reader, Writer};
use serde_json::{Map, Value};
use std::io::Cursor;

// ── JSON → XML ───────────────────────────────────────────────────────────────

/// Serialise a tree-JSON node into a full BehaviorTree.CPP v4 XML document.
pub fn value_to_xml(root: &Value) -> Result<String, String> {
    let mut w = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);
    let mut r = BytesStart::new("root");
    r.push_attribute(("BTCPP_format", "4"));
    r.push_attribute(("main_tree_to_execute", "MainTree"));
    w.write_event(Event::Start(r)).map_err(se)?;
    let mut bt = BytesStart::new("BehaviorTree");
    bt.push_attribute(("ID", "MainTree"));
    w.write_event(Event::Start(bt)).map_err(se)?;
    write_node(&mut w, root)?;
    w.write_event(Event::End(BytesEnd::new("BehaviorTree")))
        .map_err(se)?;
    w.write_event(Event::End(BytesEnd::new("root"))).map_err(se)?;
    String::from_utf8(w.into_inner().into_inner()).map_err(|e| e.to_string())
}

type Xw = Writer<Cursor<Vec<u8>>>;

fn write_node(w: &mut Xw, node: &Value) -> Result<(), String> {
    let obj = node.as_object().ok_or("tree node is not an object")?;
    let kind = obj
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("tree node is missing a `kind`")?;
    match kind {
        "sequence" => write_control(w, "Sequence", obj, &[]),
        "selector" => write_control(w, "Fallback", obj, &[]),
        "reactive_sequence" => write_control(w, "ReactiveSequence", obj, &[]),
        "reactive_selector" => write_control(w, "ReactiveFallback", obj, &[]),
        "parallel" => {
            let require = obj.get("require").and_then(Value::as_str).unwrap_or("all");
            write_control(w, "Parallel", obj, &[("require", require.to_string())])
        }
        "forever" => write_decorator(w, "Repeat", obj, &[("num_cycles", "-1".into())]),
        "repeat" => {
            let t = int_field(obj, "times");
            write_decorator(w, "Repeat", obj, &[("num_cycles", t)])
        }
        "retry" => {
            let t = int_field(obj, "times");
            write_decorator(w, "RetryUntilSuccessful", obj, &[("num_attempts", t)])
        }
        "invert" => write_decorator(w, "Inverter", obj, &[]),
        "force_success" => write_decorator(w, "ForceSuccess", obj, &[]),
        "force_failure" => write_decorator(w, "ForceFailure", obj, &[]),
        "timeout" => {
            let ms = (float_field(obj, "seconds") * 1000.0).round() as i64;
            write_decorator(w, "Timeout", obj, &[("msec", ms.to_string())])
        }
        // Cooldown has no BT.CPP-standard element; emit a custom decorator that the
        // reverse map knows, so it still round-trips (Groot shows it as a custom node).
        "cooldown" => {
            let s = float_field(obj, "seconds").to_string();
            write_decorator(w, "Cooldown", obj, &[("sec", s)])
        }
        "sub_tree" => {
            let id = obj.get("id").and_then(Value::as_str).unwrap_or_default();
            let mut e = BytesStart::new("SubTree");
            e.push_attribute(("ID", id));
            w.write_event(Event::Empty(e)).map_err(se)
        }
        _ => write_leaf(w, kind, obj),
    }
}

fn write_control(
    w: &mut Xw,
    tag: &str,
    obj: &Map<String, Value>,
    extra: &[(&str, String)],
) -> Result<(), String> {
    let mut e = BytesStart::new(tag);
    for (k, v) in extra {
        e.push_attribute((*k, v.as_str()));
    }
    w.write_event(Event::Start(e)).map_err(se)?;
    if let Some(children) = obj.get("children").and_then(Value::as_array) {
        for c in children {
            write_node(w, c)?;
        }
    }
    w.write_event(Event::End(BytesEnd::new(tag))).map_err(se)
}

fn write_decorator(
    w: &mut Xw,
    tag: &str,
    obj: &Map<String, Value>,
    extra: &[(&str, String)],
) -> Result<(), String> {
    let mut e = BytesStart::new(tag);
    for (k, v) in extra {
        e.push_attribute((*k, v.as_str()));
    }
    w.write_event(Event::Start(e)).map_err(se)?;
    if let Some(child) = obj.get("child") {
        write_node(w, child)?;
    }
    w.write_event(Event::End(BytesEnd::new(tag))).map_err(se)
}

/// Pure-condition leaves (BT.CPP `<Condition>`); every other leaf is `<Action>`.
/// The distinction is cosmetic for round-tripping — we key off `ID` either way.
const CONDITION_LEAVES: &[&str] = &["arrived", "obstacle_ahead", "facing", "path_blocked"];

fn write_leaf(w: &mut Xw, kind: &str, obj: &Map<String, Value>) -> Result<(), String> {
    let tag = if CONDITION_LEAVES.contains(&kind) {
        "Condition"
    } else {
        "Action"
    };
    let mut e = BytesStart::new(tag);
    e.push_attribute(("ID", kind));
    for (k, v) in obj {
        if k == "kind" || k == "children" || k == "child" {
            continue;
        }
        e.push_attribute((k.as_str(), attr_from_value(v).as_str()));
    }
    w.write_event(Event::Empty(e)).map_err(se)
}

/// Render a JSON scalar/array as a BT.CPP attribute string.
fn attr_from_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => {
            if a.iter().any(Value::is_array) {
                a.iter().map(attr_from_value).collect::<Vec<_>>().join("|")
            } else {
                a.iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(";")
            }
        }
        other => other.to_string(),
    }
}

fn int_field(obj: &Map<String, Value>, key: &str) -> String {
    obj.get(key)
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .to_string()
}

fn float_field(obj: &Map<String, Value>, key: &str) -> f64 {
    obj.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn se<E: std::fmt::Display>(e: E) -> String {
    format!("xml write error: {e}")
}

// ── XML → JSON ───────────────────────────────────────────────────────────────

/// A partially-built node while descending the pull-parser event stream.
struct Frame {
    tag: String,
    attrs: Vec<(String, String)>,
    children: Vec<Value>,
}

/// Parse a BehaviorTree.CPP v4 XML document into a tree-JSON node. The `<root>`
/// and `<BehaviorTree>` wrappers are transparent; the single tree they contain is
/// returned.
pub fn xml_to_value(xml: &str) -> Result<Value, String> {
    // We only act on Start/Empty/End/Eof; whitespace Text events between elements
    // fall through the `_ => {}` arm, so no text-trimming config is needed.
    let mut reader = Reader::from_str(xml);

    let mut stack: Vec<Frame> = Vec::new();
    let mut result: Option<Value> = None;

    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(e) => stack.push(Frame {
                tag: tag_name(e.name().as_ref()),
                attrs: read_attrs(&e)?,
                children: Vec::new(),
            }),
            Event::Empty(e) => {
                let tag = tag_name(e.name().as_ref());
                let node = frame_to_value(&tag, &read_attrs(&e)?, Vec::new())?;
                attach(&mut stack, &mut result, node);
            }
            Event::End(_) => {
                let f = stack.pop().ok_or("unbalanced XML")?;
                // `root`/`BehaviorTree` are transparent containers — bubble the child.
                if is_transparent(&f.tag) {
                    if let Some(c) = f.children.into_iter().next() {
                        attach(&mut stack, &mut result, c);
                    }
                    continue;
                }
                let node = frame_to_value(&f.tag, &f.attrs, f.children)?;
                attach(&mut stack, &mut result, node);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    result.ok_or_else(|| "no behaviour-tree node found in XML".into())
}

/// Push a finished node into its parent's children, or set it as the root result.
fn attach(stack: &mut [Frame], result: &mut Option<Value>, node: Value) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(node),
        None => *result = Some(node),
    }
}

fn is_transparent(tag: &str) -> bool {
    tag == "root" || tag == "BehaviorTree"
}

fn frame_to_value(
    tag: &str,
    attrs: &[(String, String)],
    children: Vec<Value>,
) -> Result<Value, String> {
    let get = |k: &str| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
    let mut m = Map::new();
    match tag {
        "Sequence" => control(&mut m, "sequence", children),
        "Fallback" => control(&mut m, "selector", children),
        "ReactiveSequence" => control(&mut m, "reactive_sequence", children),
        "ReactiveFallback" => control(&mut m, "reactive_selector", children),
        "Parallel" => {
            m.insert("kind".into(), "parallel".into());
            m.insert(
                "require".into(),
                get("require").unwrap_or("all").to_string().into(),
            );
            m.insert("children".into(), Value::Array(children));
        }
        "Repeat" => {
            let cycles: i64 = get("num_cycles").and_then(|s| s.parse().ok()).unwrap_or(1);
            if cycles < 0 {
                decorator(&mut m, "forever", children);
            } else {
                m.insert("kind".into(), "repeat".into());
                m.insert("times".into(), Value::from(cycles));
                m.insert("child".into(), one_child(children));
            }
        }
        "RetryUntilSuccessful" => {
            let n: i64 = get("num_attempts").and_then(|s| s.parse().ok()).unwrap_or(1);
            m.insert("kind".into(), "retry".into());
            m.insert("times".into(), Value::from(n));
            m.insert("child".into(), one_child(children));
        }
        "Inverter" => decorator(&mut m, "invert", children),
        "ForceSuccess" => decorator(&mut m, "force_success", children),
        "ForceFailure" => decorator(&mut m, "force_failure", children),
        "Timeout" => {
            let ms: f64 = get("msec").and_then(|s| s.parse().ok()).unwrap_or(0.0);
            m.insert("kind".into(), "timeout".into());
            m.insert("seconds".into(), Value::from(ms / 1000.0));
            m.insert("child".into(), one_child(children));
        }
        "Cooldown" => {
            let s: f64 = get("sec").and_then(|x| x.parse().ok()).unwrap_or(0.0);
            m.insert("kind".into(), "cooldown".into());
            m.insert("seconds".into(), Value::from(s));
            m.insert("child".into(), one_child(children));
        }
        "SubTree" => {
            m.insert("kind".into(), "sub_tree".into());
            m.insert("id".into(), get("ID").unwrap_or("").to_string().into());
        }
        "Action" | "Condition" => {
            let kind = get("ID").ok_or("<Action>/<Condition> missing ID")?;
            m.insert("kind".into(), kind.to_string().into());
            for (k, v) in attrs {
                if k == "ID" {
                    continue;
                }
                m.insert(k.clone(), value_from_attr(v));
            }
        }
        // Unknown element: treat as a leaf keyed by the lower-cased tag.
        other => {
            m.insert("kind".into(), snake(other).into());
            for (k, v) in attrs {
                m.insert(k.clone(), value_from_attr(v));
            }
        }
    }
    Ok(Value::Object(m))
}

fn control(m: &mut Map<String, Value>, kind: &str, children: Vec<Value>) {
    m.insert("kind".into(), kind.into());
    m.insert("children".into(), Value::Array(children));
}

fn decorator(m: &mut Map<String, Value>, kind: &str, children: Vec<Value>) {
    m.insert("kind".into(), kind.into());
    m.insert("child".into(), one_child(children));
}

fn one_child(mut children: Vec<Value>) -> Value {
    if children.is_empty() {
        Value::Null
    } else {
        children.remove(0)
    }
}

/// Recover a JSON scalar/array from a BT.CPP attribute string.
fn value_from_attr(s: &str) -> Value {
    if s == "true" {
        return Value::Bool(true);
    }
    if s == "false" {
        return Value::Bool(false);
    }
    if s.contains('|') {
        return Value::Array(s.split('|').map(value_from_attr).collect());
    }
    if s.contains(';') {
        let parts: Vec<&str> = s.split(';').collect();
        if parts.iter().all(|p| p.parse::<f64>().is_ok()) {
            return Value::Array(parts.iter().map(|p| num_value(p)).collect());
        }
        return Value::String(s.to_string());
    }
    if let Ok(i) = s.parse::<i64>() {
        return Value::from(i);
    }
    if s.parse::<f64>().is_ok() {
        return num_value(s);
    }
    Value::String(s.to_string())
}

fn num_value(s: &str) -> Value {
    if let Ok(i) = s.parse::<i64>() {
        Value::from(i)
    } else {
        serde_json::Number::from_f64(s.parse::<f64>().unwrap_or(0.0))
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

fn tag_name(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn read_attrs(e: &BytesStart) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|x| x.to_string())?;
        let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let v = a.unescape_value().map_err(|x| x.to_string())?.into_owned();
        out.push((k, v));
    }
    Ok(out)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roundtrip(v: Value) {
        let xml = value_to_xml(&v).expect("to_xml");
        let back = xml_to_value(&xml).expect("from_xml");
        assert_eq!(v, back, "\nXML was:\n{xml}");
    }

    #[test]
    fn leaf_with_vector_attr() {
        roundtrip(json!({"kind":"drive_to","target":[10.0,0.0,-5.0],"speed":0.6,"radius":2.0}));
    }

    #[test]
    fn sequence_of_leaves() {
        roundtrip(json!({
            "kind":"sequence",
            "children":[
                {"kind":"drive_to","target":[10.0,0.0,0.0],"speed":0.6,"radius":2.0},
                {"kind":"wait","seconds":1.5},
                {"kind":"brake"}
            ]
        }));
    }

    #[test]
    fn decorators_and_reactive() {
        roundtrip(json!({
            "kind":"forever",
            "child":{
                "kind":"reactive_sequence",
                "children":[
                    {"kind":"invert","child":{"kind":"arrived","target":[0.0,0.0,0.0],"radius":3.0}},
                    {"kind":"retry","times":3,"child":{"kind":"drive_to","target":[1.0,0.0,1.0],"speed":0.5,"radius":2.0}}
                ]
            }
        }));
    }

    #[test]
    fn parallel_and_timeout() {
        roundtrip(json!({
            "kind":"parallel",
            "require":"one",
            "children":[
                {"kind":"timeout","seconds":5.0,"child":{"kind":"drive_to","target":[2.0,0.0,2.0],"speed":0.6,"radius":2.0}},
                {"kind":"obstacle_ahead","distance":8.0,"cone":40.0}
            ]
        }));
    }

    #[test]
    fn emits_btcpp_wrapper() {
        let xml = value_to_xml(&json!({"kind":"brake"})).unwrap();
        assert!(xml.contains("BTCPP_format=\"4\""));
        assert!(xml.contains("<BehaviorTree ID=\"MainTree\">"));
        assert!(xml.contains("ID=\"brake\""));
    }
}
