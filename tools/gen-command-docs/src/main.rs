//! Auto-generates `docs/commands-reference.md` from every `#[Command]` struct
//! under `crates/`.
//!
//! ```sh
//! cargo run --manifest-path tools/gen-command-docs/Cargo.toml
//! ```
//!
//! It parses Rust **source** with `syn` — no app build, no reflection — so each
//! command's description comes straight from its `#[doc]` comments and the list
//! can never drift from the code. Re-run whenever commands are added or changed.
//! Undocumented commands (no `///`) are flagged in the output so they're easy to
//! find and document.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use quote::ToTokens;
use syn::{visit::Visit, Attribute, Field, File, ItemStruct};

#[derive(Clone)]
struct FieldInfo {
    name: String,
    ty: String,
    doc: String,
}

#[derive(Clone)]
struct CommandInfo {
    name: String,
    doc: String,
    fields: Vec<FieldInfo>,
    rel_file: String, // crates/<crate>/src/...rs, for traceability
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
    // Drop trailing blank doc lines.
    while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.join("\n")
}

/// True if the item carries the `#[Command]` attribute macro
/// (matches `Command`, `crate::Command`, `lunco_command_macro::Command`, …).
fn has_command_attr(attrs: &[Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| a.path().segments.last().map(|s| s.ident == "Command").unwrap_or(false))
}

struct Collector {
    rel_file: String,
    out: Vec<CommandInfo>,
}

impl<'ast> Visit<'ast> for Collector {
    fn visit_item_struct(&mut self, i: &'ast ItemStruct) {
        if has_command_attr(&i.attrs) {
            let fields: Vec<FieldInfo> = i
                .fields
                .iter()
                .filter_map(|f: &Field| {
                    let name = f.ident.as_ref()?.to_string();
                    // Normalize the token stream spacing (`f64 , x` → `f64, x`).
                    let ty = f
                        .ty
                        .to_token_stream()
                        .to_string()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .replace(" ,", ",");
                    let doc = doc_of(&f.attrs);
                    Some(FieldInfo { name, ty, doc })
                })
                .collect();
            self.out.push(CommandInfo {
                name: i.ident.to_string(),
                doc: doc_of(&i.attrs),
                fields,
                rel_file: self.rel_file.clone(),
            });
        }
        syn::visit::visit_item_struct(self, i);
    }
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
        "lunco-sandbox-edit" => ("Scene editing & authoring", 10),
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
        "lunco-terrain-surface" | "lunco-terrain-globe" | "lunco-terrain-core" => {
            ("Terrain", 91)
        }
        "lunco-obstacle-field" => ("Obstacle fields", 92),
        "lunco-materials" => ("Shaders & materials", 93),
        "lunco-api" => ("API & schema", 94),
        "lunco-core" => ("Core", 95),
        _ => ("Other", 99),
    }
}

fn main() {
    let root = repo_root();
    let crates_dir = root.join("crates");
    let out_path = root.join("docs/commands-reference.md");

    let mut files = Vec::new();
    walk_rs(&crates_dir, &mut files);

    // crate name → (set of dedup keys, list of commands)
    let mut by_crate: BTreeMap<String, Vec<CommandInfo>> = BTreeMap::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut files_scanned = 0u32;
    let mut parse_failures = 0u32;

    for f in &files {
        let rel = f
            .strip_prefix(&root)
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|_| f.clone());
        let crate_name = crate_of(&rel);
        let rel_str = rel.to_string_lossy().to_string();

        let src = match fs::read_to_string(f) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let file: File = match syn::parse_file(&src) {
            Ok(file) => file,
            Err(_) => {
                parse_failures += 1;
                continue;
            }
        };
        files_scanned += 1;

        let mut col = Collector { rel_file: rel_str, out: Vec::new() };
        col.visit_file(&file);
        for cmd in col.out {
            let key = (crate_name.clone(), cmd.name.clone());
            if !seen.insert(key) {
                continue; // same command defined twice in one crate — keep first
            }
            by_crate.entry(crate_name.clone()).or_default().push(cmd);
        }
    }

    // Sort commands alphabetically within each crate.
    for v in by_crate.values_mut() {
        v.sort_by(|a, b| a.name.cmp(&b.name));
    }

    // Crates in domain order (then alphabetical).
    let mut crates_sorted: Vec<(&String, &Vec<CommandInfo>)> = by_crate.iter().collect();
    crates_sorted.sort_by_key(|(c, _)| (domain_title(c).1, c.to_string()));

    let total_commands: usize = by_crate.values().map(|v| v.len()).sum();
    let undocumented: usize = by_crate
        .values()
        .flatten()
        .filter(|c| c.doc.is_empty())
        .count();

    // ── Emit markdown ───────────────────────────────────────────────────────
    let mut md = String::new();
    md.push_str("<!-- AUTO-GENERATED by `cargo run --manifest-path tools/gen-command-docs/Cargo.toml`.\n");
    md.push_str("     Do not edit by hand — edit the `#[doc]` on each `#[Command]` struct and re-run. -->\n\n");
    md.push_str("# Command Reference\n\n");
    md.push_str(
        "Every verb in LunCoSim is a typed `#[Command]` — an event dispatched through one\n\
         bus, reachable from the **HTTP API** (`POST /api/commands`, `{\"command\":\"…\",\"params\":{…}}`),\n\
         **MCP**, and **rhai** (`cmd(\"CommandName\", #{ … })`). This page is generated from the\n\
         command structs themselves in `crates/`, so it always matches the code. See the\n\
         [Scripting Guide](scripting-guide.md) §3 for the rhai `cmd()`/`query()` bridge and the\n\
         [API doc](architecture/12-api.md) for the HTTP contract.\n\n",
    );
    md.push_str(&format!(
        "**{total_commands} commands** across **{n_crates}** crates. ",
        n_crates = by_crate.len(),
    ));
    if undocumented > 0 {
        md.push_str(&format!(
            "{undocumented} command(s) below lack a `///` description — marked _(no description)_; \
             add a doc comment on the struct to fix it.\n\n",
        ));
    } else {
        md.push_str("All documented.\n\n");
    }
    md.push_str("> **Regenerate:** `cargo run --manifest-path tools/gen-command-docs/Cargo.toml`\n\n");

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
            "- [`{}`](#{}) (`{}`, {} command{})\n",
            crate_name,
            crate_name.replace('_', "-"),
            crate_name,
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
            md.push_str(&format!("#### `{}`\n\n", c.name));
            if c.doc.is_empty() {
                md.push_str("*(no description — add a `///` doc on the struct)*\n\n");
            } else {
                md.push_str(&format!("{}\n\n", c.doc));
            }
            md.push_str(&format!("- *defined in:* `{}`\n", c.rel_file));
            if c.fields.is_empty() {
                md.push_str(&format!("- *fields:* none — call with `{}` (no params)\n\n", c.name));
            } else {
                md.push_str("\n| Field | Type | Description |\n|---|---|---|\n");
                for f in &c.fields {
                    let desc = if f.doc.is_empty() {
                        " ".to_string()
                    } else {
                        f.doc.replace('\n', " ")
                    };
                    md.push_str(&format!("| `{}` | `{}` | {} |\n", f.name, f.ty, desc));
                }
                md.push('\n');
            }
        }
    }

    md.push_str("---\n\n");
    md.push_str(&format!(
        "<!-- scanned {files_scanned} .rs files across `crates/`; {parse_failures} parse failure(s) skipped -->\n",
    ));

    fs::write(&out_path, md).expect("write commands-reference.md");
    eprintln!(
        "wrote {} — {total_commands} commands across {} crates ({undocumented} undocumented)",
        out_path.display(),
        by_crate.len(),
    );
}
