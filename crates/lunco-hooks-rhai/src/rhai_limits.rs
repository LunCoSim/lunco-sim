//! One sandbox policy for every Rhai engine in the workspace.
//!
//! This lives in the rhai-only, bevy-free, wasm-clean leaf crate on purpose:
//! there are two independent rhai execution planes (the world-bound scripting
//! backend in `lunco-scripting-rhai-runtime`, and hook scripts compiled here),
//! and a policy that only one of them can reach is a policy that drifts. The
//! world runtime depends on this crate, so both planes use the same numbers.

use rhai::Engine;

/// Standard Rhai global expression-nesting limit, explicit to avoid dependency drift.
pub const MAX_GLOBAL_EXPR_DEPTH: usize = 64;
/// Standard Rhai function expression-nesting limit, explicit to avoid dependency drift.
pub const MAX_FUNCTION_EXPR_DEPTH: usize = 32;
/// Maximum Rhai VM operations allowed by the shared sandbox policy.
pub const MAX_OPERATIONS: u64 = 1_000_000;

/// Apply LunCoSim's bounded-resource policy to a Rhai engine.
///
/// Parser nesting, runtime operations, recursion, strings and arrays are distinct
/// limits. Keeping them together gives each execution plane identical safety and
/// authoring semantics.
pub fn apply(engine: &mut Engine) {
    engine.set_max_operations(MAX_OPERATIONS);
    engine.set_max_call_levels(64);
    engine.set_max_string_size(64 * 1024);
    engine.set_max_array_size(10_000);
    engine.set_max_expr_depths(MAX_GLOBAL_EXPR_DEPTH, MAX_FUNCTION_EXPR_DEPTH);
}
