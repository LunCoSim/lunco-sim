//! `import` for scenario scripts, over the asset pipeline.
//!
//! This module contains NO path logic. Turning an `import "…"` string into an id
//! is [`ScriptSources::canonical_id`], which is the same canonicalization USD
//! references go through — so a path means one thing everywhere, and a script
//! reached as `twin://ep1/lib.rhai` by an asset load is reached identically by an
//! import. Everything here is: ask `lunco-assets` for the id, look up the text,
//! compile it.
//!
//! # Why this must exist
//!
//! `Engine::new()` installs rhai's `FileModuleResolver`, which reads **arbitrary
//! files relative to the process working directory**. In a system that otherwise
//! routes every asset through a scoped source, that is a sandbox hole: a scenario
//! script could `import "../../../etc/passwd"`. Installing this resolver closes it
//! — nothing outside the registry is reachable, and the registry is filled only
//! from real asset sources.
//!
//! # Synchronous resolution over asynchronous loading
//!
//! [`ModuleResolver::resolve`] is synchronous and `Send + Sync`, and runs mid-tick
//! inside script evaluation; asset loading is async and, on wasm, must not block.
//! So sources are preloaded into [`ScriptSources`] and `resolve` is a pure lookup.
//! See that type's docs — `LuncoUsdResolver` solves the same problem the same way
//! for USD layers.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use lunco_assets::script_source::ScriptSources;
use rhai::{Engine, EvalAltResult, Module, ModuleResolver, Position, Scope, Shared};

/// Default extension applied to an extension-less import, so `import "lib"` and
/// `import "lib.rhai"` resolve to one id.
const SCRIPT_EXT: &str = "rhai";

/// Resolves `import` against [`ScriptSources`], memoizing compiled modules.
#[derive(Clone)]
pub struct AssetModuleResolver {
    sources: ScriptSources,
    /// Compiled-module memo, keyed by canonical id. A module imported by twenty
    /// scenarios is evaluated once.
    cache: Arc<RwLock<HashMap<String, Shared<Module>>>>,
}

impl AssetModuleResolver {
    pub fn new(sources: ScriptSources) -> Self {
        Self { sources, cache: Arc::new(RwLock::new(HashMap::new())) }
    }

    /// Drop memoized modules, so the next import recompiles from the registry.
    /// Called when a script asset hot-reloads.
    pub fn invalidate(&self) {
        if let Ok(mut c) = self.cache.write() {
            c.clear();
        }
    }
}

impl ModuleResolver for AssetModuleResolver {
    fn resolve(
        &self,
        engine: &Engine,
        source: Option<&str>,
        path: &str,
        pos: Position,
    ) -> Result<Shared<Module>, Box<EvalAltResult>> {
        // `source` is the importing script's id, which rhai threads through for
        // exactly this purpose: it is the anchor a relative import resolves against.
        let id = ScriptSources::canonical_id(path, source, SCRIPT_EXT);

        if let Some(m) = self.cache.read().ok().and_then(|c| c.get(&id).cloned()) {
            return Ok(m);
        }

        let Some(text) = self.sources.get(&id) else {
            // The detail goes to the LOG, not the error: rhai discards a resolver's
            // `ErrorModuleNotFound` payload and re-raises the miss with the raw
            // import string, so anything put in the error text is thrown away.
            //
            // It is worth logging, because the raw string is nearly useless on its
            // own — `import "lib"` failing with "lib not found" says nothing about
            // which scheme it was anchored into, and that is almost always the bug.
            // The canonical id plus what IS registered turns a guess into a diff.
            let mut known = self.sources.ids();
            let total = known.len();
            known.truncate(20);
            bevy::log::warn!(
                "[rhai] import {path:?} from {} resolved to {id}, which is not \
                 registered. {total} script(s) registered: [{}]{}",
                source.unwrap_or("<unknown>"),
                known.join(", "),
                if total > 20 { ", …" } else { "" },
            );
            return Err(Box::new(EvalAltResult::ErrorModuleNotFound(id, pos)));
        };

        // Compile and evaluate the module body. `eval_ast_as_new` RUNS the module's
        // top level, and resolution happens mid-tick inside another script — so a
        // module whose top level calls world verbs fires them at import time. Module
        // top levels are therefore expected to be definitions only; that is a rhai
        // convention, not something we can enforce here.
        let ast = engine.compile(&text).map_err(|e| {
            Box::new(EvalAltResult::ErrorInModule(id.clone(), Box::new(e.into()), pos))
        })?;
        let module = Module::eval_ast_as_new(Scope::new(), &ast, engine)
            .map_err(|e| Box::new(EvalAltResult::ErrorInModule(id.clone(), e, pos)))?;

        let shared: Shared<Module> = module.into();
        if let Ok(mut c) = self.cache.write() {
            c.insert(id, shared.clone());
        }
        Ok(shared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_with(sources: ScriptSources) -> Engine {
        let mut e = Engine::new();
        e.set_module_resolver(AssetModuleResolver::new(sources));
        e
    }

    #[test]
    fn resolves_a_registered_module() {
        let sources = ScriptSources::default();
        sources.insert("lunco://lib/math.rhai", "fn double(x) { x * 2 }");
        let engine = engine_with(sources);

        let got: i64 = engine
            .eval(r#"import "lunco://lib/math" as m; m::double(21)"#)
            .expect("import should resolve");
        assert_eq!(got, 42);
    }

    /// The reason this resolver exists: rhai's default `FileModuleResolver` would
    /// happily read this off disk relative to the process CWD.
    #[test]
    fn cannot_escape_the_registry() {
        let engine = engine_with(ScriptSources::default());
        let err = engine
            .eval::<i64>(r#"import "../../../etc/passwd" as m; 1"#)
            .unwrap_err();
        assert!(
            matches!(*err, EvalAltResult::ErrorInModule(..) | EvalAltResult::ErrorModuleNotFound(..)),
            "expected a resolution failure, got {err:?}"
        );
    }

    /// An unregistered import fails rather than falling back to anything.
    ///
    /// Note the error text carries rhai's RAW import string, not our canonical id —
    /// rhai re-raises the miss itself and discards the resolver's payload. The
    /// canonical id and the registry contents are logged instead; see `resolve`.
    #[test]
    fn unregistered_import_fails() {
        let engine = engine_with(ScriptSources::default());
        let err = engine.eval::<i64>(r#"import "twin://ep1/lib" as m; 1"#).unwrap_err();
        assert!(matches!(*err, EvalAltResult::ErrorModuleNotFound(..)), "got {err:?}");
    }

    /// Relative imports anchor to the IMPORTING script, via the shared
    /// canonicalization — no rhai-specific path handling.
    #[test]
    fn relative_import_anchors_to_the_importer() {
        let sources = ScriptSources::default();
        sources.insert("twin://ep1/lib.rhai", "fn v() { 7 }");
        let resolver = AssetModuleResolver::new(sources);
        let mut engine = Engine::new();
        engine.set_module_resolver(resolver);

        let mut ast = engine.compile(r#"import "lib" as m; m::v()"#).unwrap();
        // `source` is what rhai passes the resolver as the importing script's id.
        ast.set_source("twin://ep1/main.rhai");
        let got: i64 = engine.eval_ast(&ast).expect("relative import should resolve");
        assert_eq!(got, 7);
    }
}
