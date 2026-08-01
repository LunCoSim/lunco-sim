//! Transform editing tool.
//!
//! The tool is the Rhai policy entry point. The high-rate pointer capture,
//! floating-origin transaction, physics hold, and USD pose authoring remain in
//! the Rust editor because they are engine mechanisms rather than scenario
//! policy.

fn ui_label() { "Transform" }
fn ui_hint() { "Click an object to select it for transform editing" }

fn on_click(id) {
    cmd("SelectEntity", #{ entity_id: id, extend: false, toggle: false });
}
