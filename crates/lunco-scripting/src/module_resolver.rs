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
//! `RhaiSourceLoader` therefore declares each literal import as a normal Bevy asset
//! dependency. Once the owning scenario is ready, the event-driven publisher has
//! registered the complete dependency graph and `resolve` is a pure lookup.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use lunco_assets::script_source::ScriptSources;
use rhai::{Engine, EvalAltResult, Module, ModuleResolver, Position, Scope, Shared};

/// Default extension applied to an extension-less import, so `import "lib"` and
/// `import "lib.rhai"` resolve to one id.
const SCRIPT_EXT: &str = "rhai";

#[derive(PartialEq)]
enum ScanState {
    Code,
    LineComment,
    BlockComment,
    String(char),
}

struct ScriptScan<'a> {
    top_level_statements: Vec<&'a str>,
    imports: Vec<Result<String, String>>,
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn skip_import_trivia(source: &str, mut index: usize) -> Result<usize, String> {
    let bytes = source.as_bytes();
    loop {
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if bytes.get(index) == Some(&b'/') && bytes.get(index + 1) == Some(&b'/') {
            index += 2;
            while bytes.get(index).is_some_and(|byte| *byte != b'\n') {
                index += 1;
            }
            continue;
        }
        if bytes.get(index) == Some(&b'/') && bytes.get(index + 1) == Some(&b'*') {
            let Some(end) = source[index + 2..].find("*/") else {
                return Err("unterminated block comment after `import`".into());
            };
            index += end + 4;
            continue;
        }
        return Ok(index);
    }
}

fn parse_import_literal(source: &str, start: usize) -> Result<String, String> {
    let index = skip_import_trivia(source, start)?;
    let Some(quote) = source[index..].chars().next() else {
        return Err("missing string literal after `import`".into());
    };
    if !matches!(quote, '"' | '\'' | '`') {
        return Err("file-backed Rhai imports must use a string literal".into());
    }

    let content_start = index + quote.len_utf8();
    let mut value = String::new();
    let mut segment_start = content_start;
    let mut chars = source[content_start..].char_indices();
    while let Some((offset, character)) = chars.next() {
        let absolute = content_start + offset;
        if character == quote {
            value.push_str(&source[segment_start..absolute]);
            return Ok(value);
        }
        if character != '\\' {
            continue;
        }

        value.push_str(&source[segment_start..absolute]);
        let Some((escaped_offset, escaped)) = chars.next() else {
            return Err("unterminated escape in Rhai import path".into());
        };
        let replacement = match escaped {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '\\' => '\\',
            '"' => '"',
            '\'' => '\'',
            '`' => '`',
            other => {
                return Err(format!(
                    "unsupported escape `\\{other}` in Rhai import path"
                ));
            }
        };
        value.push(replacement);
        segment_start = content_start + escaped_offset + escaped.len_utf8();
    }

    Err("unterminated string literal after `import`".into())
}

fn scan_script(source: &str) -> ScriptScan<'_> {
    let bytes = source.as_bytes();
    let mut state = ScanState::Code;
    let mut depth = 0i32;
    let mut statement_start: Option<usize> = None;
    let mut top_level_statements = Vec::new();
    let mut imports = Vec::new();
    let mut index = 0usize;

    let starts_with = |statement: &str, keyword: &str| {
        let head = statement.trim_start();
        head.starts_with(keyword)
            && !head[keyword.len()..]
                .starts_with(|character: char| character.is_alphanumeric() || character == '_')
    };

    while index < bytes.len() {
        let byte = bytes[index];
        match state {
            ScanState::LineComment => {
                if byte == b'\n' {
                    state = ScanState::Code;
                }
            }
            ScanState::BlockComment => {
                if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    state = ScanState::Code;
                    index += 1;
                }
            }
            ScanState::String(quote) => {
                if byte == b'\\' {
                    index += 1;
                } else if byte == quote as u8 {
                    state = ScanState::Code;
                }
            }
            ScanState::Code => {
                if bytes.get(index..index + 6) == Some(b"import")
                    && (index == 0 || !is_identifier_byte(bytes[index - 1]))
                    && !bytes
                        .get(index + 6)
                        .is_some_and(|next| is_identifier_byte(*next))
                {
                    imports.push(parse_import_literal(source, index + 6));
                }

                match byte {
                    b'/' if bytes.get(index + 1) == Some(&b'/') => {
                        state = ScanState::LineComment;
                        index += 1;
                    }
                    b'/' if bytes.get(index + 1) == Some(&b'*') => {
                        state = ScanState::BlockComment;
                        index += 1;
                    }
                    b'"' | b'\'' | b'`' => {
                        if depth == 0 && statement_start.is_none() {
                            statement_start = Some(index);
                        }
                        state = ScanState::String(byte as char);
                    }
                    b'{' | b'(' | b'[' => {
                        if depth == 0 && statement_start.is_none() {
                            statement_start = Some(index);
                        }
                        depth += 1;
                    }
                    b'}' | b')' | b']' => {
                        depth -= 1;
                        if depth <= 0 {
                            depth = 0;
                            let holds = statement_start.is_some_and(|start| {
                                let statement = &source[start..=index];
                                starts_with(statement, "import") || starts_with(statement, "const")
                            });
                            if !holds {
                                statement_start = None;
                            }
                        }
                    }
                    b';' if depth == 0 => {
                        if let Some(start) = statement_start {
                            let statement = &source[start..=index];
                            if starts_with(statement, "import") || starts_with(statement, "const") {
                                top_level_statements.push(statement);
                            }
                        }
                        statement_start = None;
                    }
                    _ if depth == 0 && statement_start.is_none() && !byte.is_ascii_whitespace() => {
                        statement_start = Some(index);
                    }
                    _ => {}
                }
            }
        }
        index += 1;
    }

    ScriptScan {
        top_level_statements,
        imports,
    }
}

/// Extract the top-level `import` and `const` statements used by the hook
/// compiler. The scanner is shared with dependency discovery below so authored
/// source has one lexical interpretation in both paths.
pub(crate) fn top_level_hoist_source(source: &str) -> Option<String> {
    let statements = scan_script(source).top_level_statements;
    (!statements.is_empty()).then(|| {
        let mut output = String::new();
        for statement in statements {
            output.push_str(statement.trim());
            output.push('\n');
        }
        output
    })
}

/// Return every literal Rhai import in source order. Imports inside functions
/// are included because Rhai resolves them when that function executes; making
/// them Bevy dependencies keeps those later calls synchronous without loading
/// unrelated scripts at startup.
pub(crate) fn imported_paths(source: &str) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for result in scan_script(source).imports {
        let path = result?;
        if seen.insert(path.clone()) {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// Resolves `import` against [`ScriptSources`], memoizing compiled modules.
#[derive(Clone)]
pub struct AssetModuleResolver {
    sources: ScriptSources,
    /// Compiled-module memo, keyed by canonical id. A module imported by twenty
    /// scenarios is evaluated once.
    ///
    /// The SOURCE TEXT is stored beside the module so the memo invalidates itself:
    /// a hot-reload replaces the text in the registry, the next import sees the
    /// mismatch and recompiles. The alternative — an `invalidate()` the asset layer
    /// must remember to call — is a cache that silently serves stale code the first
    /// time someone forgets, and staleness in a scenario module is very hard to
    /// recognise from the symptom.
    cache: Arc<RwLock<HashMap<String, (String, Shared<Module>)>>>,
    /// Ids currently being evaluated, so an import cycle (A → B → A) fails with
    /// an error instead of recursing until the stack dies — the in-progress set
    /// rhai's stock `FileModuleResolver` keeps for exactly this.
    resolving: Arc<RwLock<HashSet<String>>>,
}

impl AssetModuleResolver {
    pub fn new(sources: ScriptSources) -> Self {
        Self {
            sources,
            cache: Arc::new(RwLock::new(HashMap::new())),
            resolving: Arc::new(RwLock::new(HashSet::new())),
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

        // Serve the memo only if it was compiled from the text now in the registry.
        if let Some((cached_text, m)) = self.cache.read().ok().and_then(|c| c.get(&id).cloned()) {
            if cached_text == text {
                return Ok(m);
            }
        }

        // Mark the id in-progress BEFORE evaluating: `eval_ast_as_new` re-enters
        // this resolver for the module's own imports, so a cycle would otherwise
        // recurse here until the stack dies.
        if let Ok(mut resolving) = self.resolving.write() {
            if !resolving.insert(id.clone()) {
                return Err(Box::new(EvalAltResult::ErrorInModule(
                    id.clone(),
                    Box::new(EvalAltResult::ErrorRuntime(
                        format!("import cycle detected while resolving {id}").into(),
                        pos,
                    )),
                    pos,
                )));
            }
        }

        // Compile and evaluate the module body. `eval_ast_as_new` RUNS the module's
        // top level, and resolution happens mid-tick inside another script — so a
        // module whose top level calls world verbs fires them at import time. Module
        // top levels are therefore expected to be definitions only; that is a rhai
        // convention, not something we can enforce here.
        let evaluated = engine
            .compile(&text)
            .map_err(|e| {
                Box::new(EvalAltResult::ErrorInModule(
                    id.clone(),
                    Box::new(e.into()),
                    pos,
                ))
            })
            .and_then(|ast| {
                Module::eval_ast_as_new(Scope::new(), &ast, engine)
                    .map_err(|e| Box::new(EvalAltResult::ErrorInModule(id.clone(), e, pos)))
            });
        if let Ok(mut resolving) = self.resolving.write() {
            resolving.remove(&id);
        }
        let module = evaluated?;

        let shared: Shared<Module> = module.into();
        if let Ok(mut c) = self.cache.write() {
            c.insert(id, (text, shared.clone()));
        }
        Ok(shared)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_scan_follows_all_literal_imports_without_reading_comments_or_strings() {
        let source = r#"
            // import "ignored_line";
            let text = "import \\\"ignored_string\\\";";
            import "root" as root;
            fn later() {
                /* import "ignored_block"; */
                import "nested" as nested;
                import "root" as duplicate;
            }
        "#;

        assert_eq!(imported_paths(source).unwrap(), ["root", "nested"]);
    }

    #[test]
    fn dynamic_imports_fail_dependency_discovery_instead_of_being_loaded_late() {
        let error = imported_paths("fn load(path) { import path; }").unwrap_err();
        assert!(
            error.contains("string literal"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn hoist_and_dependency_scans_share_the_same_statement_lexer() {
        let source = r#"import "root" as root; fn f() { import "nested" as nested; }"#;
        assert_eq!(
            top_level_hoist_source(source).as_deref(),
            Some("import \"root\" as root;\n")
        );
        assert_eq!(imported_paths(source).unwrap(), ["root", "nested"]);
    }

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
            matches!(
                *err,
                EvalAltResult::ErrorInModule(..) | EvalAltResult::ErrorModuleNotFound(..)
            ),
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
        let err = engine
            .eval::<i64>(r#"import "twin://ep1/lib" as m; 1"#)
            .unwrap_err();
        assert!(
            matches!(*err, EvalAltResult::ErrorModuleNotFound(..)),
            "got {err:?}"
        );
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
        let got: i64 = engine
            .eval_ast(&ast)
            .expect("relative import should resolve");
        assert_eq!(got, 7);
    }
}
