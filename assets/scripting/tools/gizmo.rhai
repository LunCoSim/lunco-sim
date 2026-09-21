//! Transform editing tool.
//!
//! The tool is the Rhai policy entry point. The high-rate pointer capture,
//! floating-origin transaction, physics hold, and USD pose authoring remain in
//! the Rust editor because they are engine mechanisms rather than scenario
//! policy.

fn ui_label() { "Transform" }
fn ui_hint() { "Click an object to select it for transform editing" }

fn on_click(context) {
    if context == () || context.button != "primary" || context.target_entity_id == () {
        return;
    }
    cmd("SelectEntity", #{
        entity_id: context.target_entity_id,
        inspector_part_entity_id: 0,
        extend: false,
        toggle: false,
        remove_only: false,
    });
}
