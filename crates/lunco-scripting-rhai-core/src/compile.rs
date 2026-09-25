//! Shared source compilation for Rhai asset and runtime preparation.

use rhai::{AST, Engine, ParseError};

/// Compile Rhai source while folding its own literal top-level constants into
/// function bodies.
///
/// Rhai functions cannot close over script-level variables. This two-pass
/// compile applies the source's literal constants as compile-scope constants,
/// preserving the authored `const NAME = value` contract for every host.
pub fn compile_with_script_consts(engine: &Engine, source: &str) -> Result<AST, ParseError> {
    let first = engine.compile(source)?;

    let mut consts = rhai::Scope::new();
    for (name, is_const, value) in first.iter_literal_variables(true, false) {
        if is_const {
            consts.push_constant_dynamic(name.to_string(), value);
        }
    }
    if consts.is_empty() {
        return Ok(first);
    }

    engine.compile_with_scope(&consts, source)
}
