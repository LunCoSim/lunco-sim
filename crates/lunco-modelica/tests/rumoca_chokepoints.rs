//! Source-scanning guards for the rumoca workaround chokepoints
//! (`docs/architecture/29-rumoca-workarounds.md`).
//!
//! A workaround only works if EVERY path goes through it. These bugs are all
//! silent — a bypassed path doesn't crash, it just quietly returns wrong numbers
//! (a frozen model clock, a demoted input, a corrupted binding). So discipline
//! and a code comment are not enough: the bypasses have to be mechanically
//! impossible to add back without a test going red.
//!
//! An audit at the 0.9.20 bump found FIVE live bypasses of the input-strip alone
//! — including the entire experiments/FastRun surface, i.e. the very feature
//! whose job is to override inputs. That is what these guards exist to prevent.

use std::path::{Path, PathBuf};

/// Every `.rs` file under a crate-relative directory.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

/// Strip `//`-comments so a chokepoint mentioned in prose (this crate documents
/// these APIs heavily) isn't mistaken for a call.
fn code_only(source: &str) -> String {
    source
        .lines()
        .map(|line| match line.find("//") {
            Some(i) => &line[..i],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// **`SimOptions` may only be constructed by the two canonical builders.**
///
/// `SimulationSession` silently clamps every `step`/`advance_to` at
/// `SimOptions::t_end`, and the `Default` is `1.0` — so a hand-rolled
/// `SimOptions` parks the model clock at t=1s and reports a frozen model as a
/// successful run (a 60 s rocket burn once drained exactly 1 s of propellant).
///
/// The only two places allowed to build one:
/// * `experiments_runner::stepper_options_from_bounds` — batch / offline / FastRun
/// * `worker::live_stepper_options` — live co-sim (`t_end = u32::MAX`, no ceiling)
///
/// Everything else — including `src/bin/` — must take options from one of those.
#[test]
fn sim_options_are_built_only_by_the_canonical_builders() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let allowed = ["experiments_runner.rs", "worker.rs"];

    let mut offenders = Vec::new();
    for file in rust_files(&src) {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        if allowed.contains(&name.as_str()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (i, line) in code_only(&text).lines().enumerate() {
            if line.contains("SimOptions::default()") || line.contains("SimOptions {") {
                offenders.push(format!("{}:{}: {}", file.display(), i + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "SimOptions must come from `stepper_options_from_bounds` (batch) or \
         `live_stepper_options` (live) — a hand-rolled one inherits t_end=1.0 and \
         silently freezes the model clock. See docs/architecture/29-rumoca-workarounds.md §1.\n\
         Offending sites:\n  {}",
        offenders.join("\n  ")
    );
}

/// **No production path may re-emit source through rumoca's `to_modelica()`.**
///
/// The emitter is a lossy round-trip: it drops comments, and it conflates a
/// component's `start` modifier with its declaration binding — so
/// `parameter Real m(start = 1, min = 0) = 5` comes back as `m(min = 0) = 1`
/// and `parameter Real k = 2.0` as `k = 0.0`. Structural ops used to rebuild a
/// whole class with it, which meant dragging one icon on the canvas silently
/// rewrote every other declaration in that class. Wrong numbers, no error.
///
/// Ops now splice: they rewrite the bytes they own and copy the rest of the
/// source through untouched (`ast_mut/edit.rs`). Nothing in `src/` may call the
/// emitter again. See docs/architecture/29-rumoca-workarounds.md §5.
#[test]
fn source_is_never_regenerated_through_the_rumoca_emitter() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    let mut offenders = Vec::new();
    for file in rust_files(&src) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (i, line) in code_only(&text).lines().enumerate() {
            if line.contains(".to_modelica(") {
                offenders.push(format!("{}:{}: {}", file.display(), i + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "rumoca's `to_modelica()` emitter must not be used to produce source: it drops \
         comments and corrupts declarations that carry both a `start` modifier and a \
         binding. Author the bytes you mean to change as a splice (`ast_mut::Edit`), and \
         render genuinely NEW nodes with `crate::pretty`. \
         See docs/architecture/29-rumoca-workarounds.md §5.\n\
         Offending sites:\n  {}",
        offenders.join("\n  ")
    );
}

/// **User model source enters rumoca only through `seat_user_source`.**
///
/// rumoca demotes a bound `input Real g = 9.81` to an algebraic, so it never
/// reaches `input_names()` and every `set_input` on it fails. The strip lives
/// inside `ModelicaCompiler::compile_str` / `compile_str_multi`; any code that
/// calls `session.update_document(...)` or `session.add_document(...)` directly
/// on the COMPILE session seats unstripped text and re-opens the bug.
///
/// (`engine.rs` / `indexer.rs` / `class_cache.rs` run a *different*, query-side
/// session that never produces a DAE, so they are exempt.)
#[test]
fn user_source_is_seated_only_through_the_strip_chokepoint() {
    let lib_rs = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
    let text = std::fs::read_to_string(&lib_rs).expect("lib.rs readable");

    let seats: Vec<String> = code_only(&text)
        .lines()
        .enumerate()
        .filter(|(_, l)| {
            l.contains("session.update_document(") || l.contains("session.add_document(")
        })
        .map(|(i, l)| format!("lib.rs:{}: {}", i + 1, l.trim()))
        .collect();

    // Exactly two are expected:
    //   * `seat_user_source` — the chokepoint itself (strips first)
    //   * `load_source_root_in_memory` — library roots, NOT user model source
    //     (documented hole: §3 of the workarounds doc — untracked in
    //     `seated_user_uris`, so it can't be evicted)
    assert_eq!(
        seats.len(),
        2,
        "a new site seats documents into the COMPILE session. User model source must go \
         through `seat_user_source` (which applies strip_input_defaults) — otherwise bound \
         inputs are silently demoted. See docs/architecture/29-rumoca-workarounds.md §2.\n\
         Sites found:\n  {}",
        seats.join("\n  ")
    );
}
