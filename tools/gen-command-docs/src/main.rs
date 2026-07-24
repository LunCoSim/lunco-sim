//! Generates `docs/commands-reference.md` from the **runtime** command schema.
//!
//! ```sh
//! # 1. dump the schema from a running app (headless is fine):
//! cargo run -p lunco-sandbox-server -- --api --no-ui &
//! curl -s http://127.0.0.1:4101/api/commands/schema > /tmp/schema.json
//! # 2. generate:
//! cargo run -p gen-command-docs -- --schema /tmp/schema.json
//! ```
//!
//! # Why a schema, not a grep
//!
//! The previous version text-scraped `#[Command]` structs out of `crates/**.rs`
//! with `syn` and called that the command list. It was wrong in both directions:
//! it MISSED commands (a `#[Command]` behind a `macro_rules!` or a re-export
//! never matched the pattern) and it INVENTED them — it published `TestEcho`, a
//! `#[cfg(test)]` unit-test fixture, as public API, and it could not see
//! `ApiVisibility::hide`, so internal-only verbs leaked into the doc too.
//!
//! `DiscoverSchema` is the same derived, visibility-filtered list that drives the
//! MCP tool surface and the API itself. Making it the source here means the doc
//! can only ever describe commands that a caller can actually call.
//!
//! Source is still parsed — but only for *prose*: the `///` doc comments and the
//! defining file, neither of which survives into the reflect schema. A command in
//! the schema with no source match still gets documented (name + fields); a
//! `#[Command]` in source that is NOT in the schema is deliberately omitted and
//! listed in a trailing HTML comment, because it is not reachable.
//!
//! A host-side dev tool: it reads `.rs` files and writes a `.md` file, and never
//! runs on wasm. The workspace `disallowed_methods` ban on `std::fs` exists to
//! keep the *browser* build from calling a wasm-panicking API; it does not apply
//! here (`clippy.toml`'s header says so, but cargo has no path-scoped lint
//! config, so the exemption has to be written out).
#![allow(clippy::disallowed_methods)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use quote::ToTokens;
use serde::Deserialize;
use syn::{visit::Visit, Attribute, Field, ItemStruct};

// ── The runtime schema (`DiscoverSchema`) ───────────────────────────────────

/// One command as the app itself reports it. Field order/types come from
/// `bevy_reflect`, so they are what the deserializer actually accepts.
#[derive(Debug, Deserialize)]
struct SchemaCommand {
    name: String,
    #[serde(default)]
    fields: Vec<SchemaField>,
}

#[derive(Debug, Deserialize)]
struct SchemaField {
    name: String,
    type_name: String,
}

/// The response body of `GET /api/commands/schema` (or a `DiscoverSchema`
/// `POST /api/commands`). Both wrap the payload in the API envelope
/// (`{"data": {"commands": [...]}}`); a bare `{"commands": [...]}` is accepted
/// too, so a hand-dumped schema works.
#[derive(Debug, Deserialize)]
struct SchemaEnvelope {
    data: Option<ApiSchema>,
    commands: Option<Vec<SchemaCommand>>,
}

#[derive(Debug, Deserialize)]
struct ApiSchema {
    commands: Vec<SchemaCommand>,
}

fn load_schema(path: &Path) -> Vec<SchemaCommand> {
    let raw = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read schema `{}`: {e}", path.display()));
    let env: SchemaEnvelope = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("`{}` is not a DiscoverSchema response: {e}", path.display()));
    env.data
        .map(|d| d.commands)
        .or(env.commands)
        .unwrap_or_else(|| panic!("`{}` has no `commands` array", path.display()))
}

// ── Source scrape (prose only) ──────────────────────────────────────────────

#[derive(Clone, Default)]
struct FieldDoc {
    ty: String,
    doc: String,
}

#[derive(Clone)]
struct SourceInfo {
    doc: String,
    /// field name → its `///` doc + written-out Rust type.
    fields: BTreeMap<String, FieldDoc>,
    rel_file: String,
    crate_name: String,
}

/// Repo root = two levels up from this package's manifest dir.
fn repo_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Join a command/field's `#[doc = "..."]` attrs into one trimmed block.
fn doc_of(attrs: &[Attribute]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for a in attrs {
        if !a.path().is_ident("doc") {
            continue;
        }
        if let Ok(nv) = a.meta.require_name_value() {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            {
                lines.push(s.value().trim_end().to_string());
            }
        }
    }
    while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.join("\n")
}

/// True if the item carries the `#[Command]` attribute macro
/// (matches `Command`, `crate::Command`, `lunco_command_macro::Command`, …).
fn has_command_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path()
            .segments
            .last()
            .map(|s| s.ident == "Command")
            .unwrap_or(false)
    })
}

struct Collector {
    rel_file: String,
    crate_name: String,
    out: Vec<(String, SourceInfo)>,
}

impl<'ast> Visit<'ast> for Collector {
    fn visit_item_struct(&mut self, i: &'ast ItemStruct) {
        if has_command_attr(&i.attrs) {
            let mut fields = BTreeMap::new();
            for f in i.fields.iter() {
                let Some(name) = f.ident.as_ref().map(|n| n.to_string()) else {
                    continue;
                };
                fields.insert(name, FieldDoc { ty: type_str(f), doc: doc_of(&f.attrs) });
            }
            self.out.push((
                i.ident.to_string(),
                SourceInfo {
                    doc: doc_of(&i.attrs),
                    fields,
                    rel_file: self.rel_file.clone(),
                    crate_name: self.crate_name.clone(),
                },
            ));
        }
        syn::visit::visit_item_struct(self, i);
    }
}

/// Normalize a field's token stream spacing (`f64 , x` → `f64, x`).
fn type_str(f: &Field) -> String {
    f.ty.to_token_stream()
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" ,", ",")
}

/// Recursively collect `.rs` files, skipping `target/`.
fn walk_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().and_then(|n| n.to_str()) != Some("target") {
                walk_rs(&p, out);
            }
        } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

/// `crates/<crate>/src/foo.rs` → `<crate>`.
fn crate_of(rel: &Path) -> String {
    let mut it = rel.components();
    if it.next().and_then(|c| match c {
        Component::Normal(s) => Some(s.to_str().unwrap_or("").to_string()),
        _ => None,
    }) != Some("crates".to_string())
    {
        return "?".to_string();
    }
    it.next()
        .and_then(|c| match c {
            Component::Normal(s) => s.to_str().map(|s| s.to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "?".to_string())
}

/// Friendly domain title per crate, with a stable display order.
fn domain_title(crate_name: &str) -> (&'static str, u32) {
    match crate_name {
        "lunco-scene-commands" | "lunco-sandbox-edit" => ("Scene editing & authoring", 10),
        "lunco-usd" => ("USD / scenes", 11),
        "lunco-usd-bevy" | "lunco-usd-sim" | "lunco-usd-avian" => ("USD / scenes", 12),
        "lunco-modelica" => ("Modelica modeling & simulation", 20),
        "lunco-cosim" => ("Co-simulation", 21),
        "lunco-mobility" | "lunco-hardware" | "lunco-controller" | "lunco-autopilot" => {
            ("Vessels, mobility & control", 30)
        }
        "lunco-avatar" => ("Avatar & possession", 31),
        "lunco-workbench" | "lunco-ui" => ("Workbench UI & panels", 40),
        "lunco-tutorial" => ("Tutorials & HUD", 41),
        "lunco-scripting" | "lunco-tools-rhai" => ("Scripting & scenarios", 50),
        "lunco-doc-bevy" | "lunco-doc" | "lunco-twin" | "lunco-twin-journal" => {
            ("Documents & twins", 60)
        }
        "lunco-experiments" => ("Experiments & sweeps", 61),
        "lunco-networking" => ("Networking", 70),
        "lunco-time" => ("Time & clock", 80),
        "lunco-celestial" | "lunco-celestial-ephemeris" | "lunco-environment" => {
            ("Celestial, environment & comms", 90)
        }
        "lunco-terrain-surface" | "lunco-terrain-globe" | "lunco-terrain-core" => ("Terrain", 91),
        "lunco-obstacle-field" => ("Obstacle fields", 92),
        "lunco-materials" => ("Shaders & materials", 93),
        "lunco-api" => ("API & schema", 94),
        "lunco-core" => ("Core", 95),
        _ => ("Other (source location unknown)", 99),
    }
}

/// The reflect type path is fully qualified (`alloc::string::String`,
/// `bevy_ecs::entity::Entity`). Show the last segment — that's what a caller
/// reads — but keep generics intact.
fn short_type(path: &str) -> String {
    match path.rsplit("::").next() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => path.to_string(),
    }
}

const USAGE: &str = "\
usage: cargo run -p gen-command-docs -- --schema <a.json> [--schema <b.json> ...]

  Each <schema.json> is a `DiscoverSchema` response from a RUNNING app — the
  authoritative, visibility-filtered command list (the same one MCP reads).

  PASS BOTH DUMPS. No single app registers every command:
    - the headless server has no workbench, so no `CaptureScreenshot`;
    - a GUI build has none of the server-only verbs.
  Generating from one dump alone silently DELETES the other's commands from the
  reference.

      # headless
      cargo run -p lunco-sandbox-server -- --api --no-ui &
      curl -s http://127.0.0.1:4101/api/commands/schema > /tmp/schema-server.json
      # GUI (needs a display)
      cargo run -p lunco-sandbox -- --api &
      curl -s http://127.0.0.1:4101/api/commands/schema > /tmp/schema-gui.json

      cargo run -p gen-command-docs -- --schema /tmp/schema-server.json --schema /tmp/schema-gui.json

  There is deliberately no source-scrape fallback: a grep of `#[Command]` both
  misses commands and invents them (it used to publish the `TestEcho` unit-test
  fixture as public API), and a doc that is confidently wrong is worse than one
  that refuses to build.
";

fn main() {
    let mut args = std::env::args().skip(1);
    // MULTIPLE schemas, unioned. No single running app knows every command: a headless
    // server has no workbench (so no `CaptureScreenshot`), and a GUI build has no
    // server-only verbs. Generating from one dump alone silently DELETES the other's
    // commands from this reference — which is how a doc that is confidently wrong gets made.
    let mut schema_paths: Vec<PathBuf> = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--schema" => schema_paths.extend(args.next().map(PathBuf::from)),
            "-h" | "--help" => {
                eprint!("{USAGE}");
                return;
            }
            other => {
                eprintln!("unknown argument `{other}`\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    if schema_paths.is_empty() {
        eprint!("{USAGE}");
        std::process::exit(2);
    }

    let root = repo_root();
    let out_path = root.join("docs/commands-reference.md");

    // 1. The authoritative list — the UNION of every dump, deduped by name. First definition
    //    of a name wins (they agree; a command is the same type wherever it is registered).
    let mut seen = std::collections::HashSet::new();
    let mut schema: Vec<SchemaCommand> = Vec::new();
    for path in &schema_paths {
        for cmd in load_schema(path) {
            if seen.insert(cmd.name.clone()) {
                schema.push(cmd);
            }
        }
    }
    eprintln!(
        "[gen-command-docs] {} commands from {} schema dump(s)",
        schema.len(),
        schema_paths.len()
    );
    schema.sort_by(|a, b| a.name.cmp(&b.name));

    // 2. The prose, scraped from source and joined by name.
    let mut files = Vec::new();
    walk_rs(&root.join("crates"), &mut files);
    let mut source: BTreeMap<String, SourceInfo> = BTreeMap::new();
    let mut files_scanned = 0u32;
    let mut parse_failures = 0u32;
    for f in &files {
        let rel = f
            .strip_prefix(&root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| f.clone());
        let Ok(src) = fs::read_to_string(f) else { continue };
        let Ok(file) = syn::parse_file(&src) else {
            parse_failures += 1;
            continue;
        };
        files_scanned += 1;
        let mut col = Collector {
            rel_file: rel.to_string_lossy().to_string(),
            crate_name: crate_of(&rel),
            out: Vec::new(),
        };
        col.visit_file(&file);
        for (name, info) in col.out {
            source.entry(name).or_insert(info);
        }
    }

    // 3. Join. The schema decides WHAT exists; source only decorates it.
    let schema_names: BTreeSet<&str> = schema.iter().map(|c| c.name.as_str()).collect();
    let unreachable: Vec<&String> = source
        .keys()
        .filter(|n| !schema_names.contains(n.as_str()))
        .collect();
    let undocumented: Vec<&str> = schema
        .iter()
        .map(|c| c.name.as_str())
        .filter(|n| source.get(*n).map(|s| s.doc.is_empty()).unwrap_or(true))
        .collect();
    let unknown_source: Vec<&str> = schema
        .iter()
        .map(|c| c.name.as_str())
        .filter(|n| !source.contains_key(*n))
        .collect();

    // Group by the crate the command is DEFINED in (schema doesn't carry that).
    let mut by_crate: BTreeMap<String, Vec<&SchemaCommand>> = BTreeMap::new();
    for c in &schema {
        let crate_name = source
            .get(&c.name)
            .map(|s| s.crate_name.clone())
            .unwrap_or_else(|| "?".to_string());
        by_crate.entry(crate_name).or_default().push(c);
    }
    let mut crates_sorted: Vec<(&String, &Vec<&SchemaCommand>)> = by_crate.iter().collect();
    crates_sorted.sort_by_key(|(c, _)| (domain_title(c).1, c.to_string()));

    // ── Emit markdown ───────────────────────────────────────────────────────
    let total = schema.len();
    let mut md = String::new();
    md.push_str("<!-- AUTO-GENERATED. Do not edit by hand.\n");
    md.push_str("     Source of truth: the running app's `DiscoverSchema` (GET /api/commands/schema),\n");
    md.push_str("     decorated with the `///` docs on each `#[Command]` struct.\n");
    md.push_str("     Regenerate: cargo run -p gen-command-docs -- --schema <schema.json> -->\n\n");
    md.push_str("# Command Reference\n\n");
    md.push_str(
        "Every verb in LunCoSim is a typed `#[Command]` — an event dispatched through one\n\
         bus, reachable from the **HTTP API** (`POST /api/commands`, `{\"command\":\"…\",\"params\":{…}}`),\n\
         **MCP**, and **rhai** (`cmd(\"CommandName\", #{ … })`). This page is generated from the\n\
         **runtime schema** the app itself advertises, so every command below is one you can\n\
         actually call, with the fields the deserializer actually accepts. See the\n\
         [Scripting Guide](scripting-guide.md) §3 for the rhai `cmd()`/`query()` bridge and the\n\
         [API doc](architecture/12-api.md) for the HTTP contract.\n\n",
    );
    md.push_str(&format!(
        "**{total} commands** across **{}** crates. ",
        by_crate.len()
    ));
    if undocumented.is_empty() {
        md.push_str("All documented.\n\n");
    } else {
        md.push_str(&format!(
            "{} command(s) lack a `///` description — marked _(no description)_ below, and shown \
             the same way in the MCP tool list an agent reads; add a doc comment on the struct to \
             fix it.\n\n",
            undocumented.len()
        ));
    }
    md.push_str(
        "> **Regenerate:** dump the schema from a running app, then\n\
         > `cargo run -p gen-command-docs -- --schema <schema.json>` (see the tool's `--help`).\n\n",
    );

    // Quick-jump index.
    md.push_str("## Index\n\n");
    let mut last_domain: Option<&'static str> = None;
    for (crate_name, cmds) in &crates_sorted {
        let (domain, _) = domain_title(crate_name);
        if last_domain != Some(domain) {
            md.push_str(&format!("\n**{domain}**\n\n"));
            last_domain = Some(domain);
        }
        md.push_str(&format!(
            "- [`{}`](#{}) ({} command{})\n",
            crate_name,
            crate_name.replace('_', "-"),
            cmds.len(),
            if cmds.len() == 1 { "" } else { "s" },
        ));
    }
    md.push('\n');

    // Per-crate sections.
    md.push_str("---\n\n");
    let mut last_domain: Option<&'static str> = None;
    for (crate_name, cmds) in &crates_sorted {
        let (domain, _) = domain_title(crate_name);
        if last_domain != Some(domain) {
            md.push_str(&format!("## {domain}\n\n"));
            last_domain = Some(domain);
        }
        md.push_str(&format!(
            "### `{}` <a id=\"{}\"></a>\n\n",
            crate_name,
            crate_name.replace('_', "-"),
        ));
        for c in cmds.iter() {
            let src = source.get(&c.name);
            md.push_str(&format!("#### `{}`\n\n", c.name));
            match src.map(|s| s.doc.as_str()).unwrap_or("") {
                "" => md.push_str("*(no description — add a `///` doc on the struct)*\n\n"),
                doc => md.push_str(&format!("{doc}\n\n")),
            }
            if let Some(s) = src {
                md.push_str(&format!("- *defined in:* `{}`\n", s.rel_file));
            }
            if c.fields.is_empty() {
                md.push_str(&format!(
                    "- *fields:* none — call with `{}` (no params)\n\n",
                    c.name
                ));
            } else {
                md.push_str("\n| Field | Type | Description |\n|---|---|---|\n");
                for f in &c.fields {
                    let fd = src.and_then(|s| s.fields.get(&f.name));
                    // Prefer the written-out Rust type from source (`Option<f64>`
                    // reads better than the reflect path); fall back to the
                    // schema's, which is always present.
                    let ty = fd
                        .map(|d| d.ty.clone())
                        .unwrap_or_else(|| short_type(&f.type_name));
                    let desc = fd
                        .map(|d| d.doc.replace('\n', " "))
                        .unwrap_or_default();
                    let desc = if desc.trim().is_empty() { " ".into() } else { desc };
                    md.push_str(&format!("| `{}` | `{}` | {} |\n", f.name, ty, desc));
                }
                md.push('\n');
            }
        }
    }

    md.push_str("---\n\n");
    md.push_str(&format!(
        "<!-- {total} commands from the runtime schema; scanned {files_scanned} .rs files for docs \
         ({parse_failures} parse failure(s) skipped).\n"
    ));
    if !unknown_source.is_empty() {
        md.push_str(&format!(
            "     In the schema but no `#[Command]` struct found in source (macro-generated?): {}\n",
            unknown_source.join(", ")
        ));
    }
    if !unreachable.is_empty() {
        md.push_str(&format!(
            "     `#[Command]` in source but NOT in the runtime schema — test fixtures, hidden\n\
             \x20    (`ApiVisibility::hide`), or never registered; deliberately not documented: {}\n",
            unreachable
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    md.push_str("-->\n");

    fs::write(&out_path, md).expect("write commands-reference.md");
    eprintln!(
        "wrote {} — {total} commands across {} crates",
        out_path.display(),
        by_crate.len()
    );
    if !undocumented.is_empty() {
        eprintln!(
            "\n{} command(s) with NO doc comment (they render as `_(no description)_` in the MCP \
             tool list an agent reads):\n  {}",
            undocumented.len(),
            undocumented.join("\n  ")
        );
    }
    if !unreachable.is_empty() {
        eprintln!(
            "\n{} `#[Command]` struct(s) in source are NOT in the runtime schema (hidden, \
             test-only, or unregistered) and were omitted:\n  {}",
            unreachable.len(),
            unreachable
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        );
    }
}
