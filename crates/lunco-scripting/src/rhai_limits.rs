//! One sandbox policy for every Rhai engine created by this crate.

use rhai::Engine;

/// Standard Rhai global expression-nesting limit, explicit to avoid dependency drift.
pub const MAX_GLOBAL_EXPR_DEPTH: usize = 64;
/// Standard Rhai function expression-nesting limit, explicit to avoid dependency drift.
pub const MAX_FUNCTION_EXPR_DEPTH: usize = 32;

/// Apply LunCoSim's bounded-resource policy to a Rhai engine.
///
/// Parser nesting, runtime operations, recursion, strings and arrays are distinct
/// limits. Keeping them together gives each execution plane identical safety and
/// authoring semantics.
pub fn apply(engine: &mut Engine) {
    engine.set_max_operations(1_000_000);
    engine.set_max_call_levels(64);
    engine.set_max_string_size(64 * 1024);
    engine.set_max_array_size(10_000);
    engine.set_max_expr_depths(MAX_GLOBAL_EXPR_DEPTH, MAX_FUNCTION_EXPR_DEPTH);
}
