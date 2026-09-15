// Negative fixture for the authored `lint.rhai` policy.
// This is syntactically valid Rhai, but its behavior is coupled to the Rust
// build profile and must be rejected by the replaceable policy.
let attended = is_debug();
