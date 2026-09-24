//! `import` for scenario scripts, over the asset pipeline.
//!
//! This module contains NO path logic. Turning an `import "…"` string into an id
//! is [`ScriptSources::canonical_id`], which is the same canonicalization USD
//! references go through — so a path means one thing everywhere, and a script
//! reached as `twin://ep1/lib.rhai` by an asset load is reached identically by an
//! import. Preparation resolves each literal import through that registry and
//! compiles source-backed modules off-thread; the scenario resolver looks up
//! the committed AST and evaluates the module body.
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
//! registered the complete dependency graph. Scenario `resolve` reads only the
//! source registry and owner-committed AST cache; it does not compile module
//! source on the lifecycle path.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, RwLock};

use lunco_assets_runtime::script_source::ScriptSources;
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
pub fn top_level_hoist_source(source: &str) -> Option<String> {
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
pub fn imported_paths(source: &str) -> Result<Vec<String>, String> {
    let (paths, error) = discover_import_paths(source);
    error.map_or(Ok(paths), Err)
}

fn discover_import_paths(source: &str) -> (Vec<String>, Option<String>) {
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    let mut error = None;
    for result in scan_script(source).imports {
        match result {
            Ok(path) if seen.insert(path.clone()) => paths.push(path),
            Ok(_) => {}
            Err(message) => {
                error.get_or_insert(message);
            }
        }
    }
    (paths, error)
}

/// One source-backed import observed while preparing a Rhai program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetModuleDependency {
    /// Canonical source id used by the runtime resolver.
    pub id: String,
    /// Source text in the immutable preparation snapshot, or `None` when the
    /// import belongs to another registered resolver or is currently missing.
    pub source: Option<String>,
}

/// Transitive source-backed imports discovered from one immutable source set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetImportClosure {
    /// All canonical imports, including unresolved names handled by another
    /// registered resolver. Ordered by canonical id for stable validation.
    pub dependencies: Vec<AssetModuleDependency>,
    /// Loaded source-backed module bodies, ordered by canonical id.
    pub modules: Vec<(String, String)>,
}

/// Import-discovery failure with every dependency identified before the error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetImportClosureError {
    pub message: String,
    pub dependencies: Vec<AssetModuleDependency>,
}

/// Owner-thread cache of ASTs prepared away from the evaluation boundary.
#[derive(Clone, Default)]
pub struct PreparedModuleAsts {
    modules: Arc<RwLock<HashMap<String, (String, rhai::AST)>>>,
    require_prepared: bool,
}

impl PreparedModuleAsts {
    /// Create a cache whose consumer must receive every source AST through
    /// owner-thread commit from immutable background preparation.
    pub fn required() -> Self {
        Self {
            modules: Arc::new(RwLock::new(HashMap::new())),
            require_prepared: true,
        }
    }

    /// Commit one source-matched immutable module AST.
    pub fn insert(&self, id: String, source: String, ast: rhai::AST) {
        self.modules
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, (source, ast));
    }

    fn get(&self, id: &str, source: &str) -> Option<rhai::AST> {
        self.modules
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .filter(|(prepared_source, _)| prepared_source == source)
            .map(|(_, ast)| ast.clone())
    }
}

/// Resolve the loaded source-backed portion of a program's literal import
/// graph. The caller supplies an immutable source snapshot; this function does
/// no asset I/O and never evaluates module bodies.
pub fn asset_import_closure(
    root_source: &str,
    root_id: Option<&str>,
    sources: &BTreeMap<String, String>,
) -> Result<AssetImportClosure, AssetImportClosureError> {
    fn visit(
        source: &str,
        importer: Option<&str>,
        sources: &BTreeMap<String, String>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
        dependencies: &mut BTreeMap<String, Option<String>>,
        modules: &mut BTreeMap<String, String>,
        errors: &mut Vec<String>,
    ) {
        let (paths, discovery_error) = discover_import_paths(source);
        if let Some(error) = discovery_error {
            errors.push(error);
        }
        for path in paths {
            let id = ScriptSources::canonical_id(&path, importer, SCRIPT_EXT);
            let dependency_source = sources.get(&id).cloned();
            dependencies
                .entry(id.clone())
                .or_insert_with(|| dependency_source.clone());
            let Some(dependency_source) = dependency_source else {
                continue;
            };
            if visited.contains(&id) {
                continue;
            }
            if !visiting.insert(id.clone()) {
                errors.push(format!("Rhai import cycle detected at {id}"));
                continue;
            }
            visit(
                &dependency_source,
                Some(&id),
                sources,
                visiting,
                visited,
                dependencies,
                modules,
                errors,
            );
            visiting.remove(&id);
            visited.insert(id.clone());
            modules.insert(id, dependency_source);
        }
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut dependencies = BTreeMap::new();
    let mut modules = BTreeMap::new();
    let mut errors = Vec::new();
    visit(
        root_source,
        root_id,
        sources,
        &mut visiting,
        &mut visited,
        &mut dependencies,
        &mut modules,
        &mut errors,
    );
    let closure = AssetImportClosure {
        dependencies: dependencies
            .into_iter()
            .map(|(id, source)| AssetModuleDependency { id, source })
            .collect(),
        modules: modules.into_iter().collect(),
    };
    if let Some(message) = errors.into_iter().next() {
        Err(AssetImportClosureError {
            message,
            dependencies: closure.dependencies,
        })
    } else {
        Ok(closure)
    }
}

/// Resolves `import` against [`ScriptSources`], memoizing compiled modules.
#[derive(Clone)]
pub struct AssetModuleResolver {
    sources: ScriptSources,
    prepared: PreparedModuleAsts,
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
        Self::with_prepared_modules(sources, PreparedModuleAsts::default())
    }

    /// Build a resolver that consumes owner-committed module AST preparation.
    pub fn with_prepared_modules(sources: ScriptSources, prepared: PreparedModuleAsts) -> Self {
        Self {
            sources,
            prepared,
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

        // Scenario engines provide a worker-prepared AST; generic one-shot
        // engines may compile here. `eval_ast_as_new` RUNS the module's top level,
        // and resolution happens inside another script, so module top-level world
        // calls execute at import time. Module top levels are therefore expected
        // to be definitions only; Rhai does not enforce that convention.
        let ast = match self.prepared.get(&id, &text) {
            Some(ast) => Ok(ast),
            None if self.prepared.require_prepared => Err(Box::new(EvalAltResult::ErrorInModule(
                id.clone(),
                Box::new(EvalAltResult::ErrorRuntime(
                    format!("asset module {id} was not prepared for this scenario revision").into(),
                    pos,
                )),
                pos,
            ))),
            None => engine.compile(&text).map_err(|error| {
                Box::new(EvalAltResult::ErrorInModule(
                    id.clone(),
                    Box::new(error.into()),
                    pos,
                ))
            }),
        };
        let evaluated = ast.and_then(|ast| {
            Module::eval_ast_as_new(Scope::new(), &ast, engine)
                .map_err(|error| Box::new(EvalAltResult::ErrorInModule(id.clone(), error, pos)))
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

    #[test]
    fn asset_import_closure_is_transitive_sorted_and_records_unresolved_modules() {
        let sources = [
            (
                "twin://mission/z.rhai".to_owned(),
                "import \"nested\" as nested; fn value() { nested::value() }".to_owned(),
            ),
            (
                "twin://mission/nested.rhai".to_owned(),
                "fn value() { 42 }".to_owned(),
            ),
        ]
        .into_iter()
        .collect();

        let closure = asset_import_closure(
            r#"import "z" as z; import "registered_tool" as tool; fn main() { z::value() }"#,
            Some("twin://mission/main.rhai"),
            &sources,
        )
        .expect("literal imports should produce a source closure");

        assert_eq!(
            closure
                .modules
                .iter()
                .map(|module| module.0.as_str())
                .collect::<Vec<_>>(),
            ["twin://mission/nested.rhai", "twin://mission/z.rhai"]
        );
        assert_eq!(
            closure
                .dependencies
                .iter()
                .map(|dependency| (dependency.id.as_str(), dependency.source.is_some()))
                .collect::<Vec<_>>(),
            [
                ("twin://mission/nested.rhai", true),
                ("twin://mission/registered_tool.rhai", false),
                ("twin://mission/z.rhai", true),
            ]
        );
    }

    #[test]
    fn asset_import_closure_rejects_cycles_before_module_evaluation() {
        let sources = [
            (
                "twin://mission/a.rhai".to_owned(),
                "import \"b\" as b;".to_owned(),
            ),
            (
                "twin://mission/b.rhai".to_owned(),
                "import \"a\" as a;".to_owned(),
            ),
        ]
        .into_iter()
        .collect();

        let error = asset_import_closure(
            r#"import "a" as a;"#,
            Some("twin://mission/main.rhai"),
            &sources,
        )
        .expect_err("cyclic imports must fail during immutable preparation");
        assert!(
            error.message.contains("import cycle"),
            "unexpected error: {error:?}"
        );
        assert_eq!(
            error
                .dependencies
                .iter()
                .map(|dependency| dependency.id.as_str())
                .collect::<Vec<_>>(),
            ["twin://mission/a.rhai", "twin://mission/b.rhai"]
        );
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

    #[test]
    fn prepared_only_resolver_rejects_an_unprepared_asset_module() {
        let sources = ScriptSources::default();
        sources.insert("lunco://lib/math.rhai", "fn double(x) { x * 2 }");
        let mut engine = Engine::new();
        engine.set_module_resolver(AssetModuleResolver::with_prepared_modules(
            sources,
            PreparedModuleAsts::required(),
        ));

        let error = engine
            .eval::<i64>(r#"import "lunco://lib/math" as math; math::double(21)"#)
            .expect_err("scenario imports must not compile inside evaluation");
        assert!(
            error
                .to_string()
                .contains("was not prepared for this scenario revision"),
            "expected the missing-preparation diagnostic, got {error:?}"
        );
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
