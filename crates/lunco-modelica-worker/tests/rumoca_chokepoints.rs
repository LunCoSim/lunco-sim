//! Source ownership checks for the Modelica runtime construction boundaries.

use std::path::{Path, PathBuf};

/// Every `.rs` file under a crate-relative directory.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = lunco_storage::read_directory_sync(dir) else {
        return out;
    };
    for path in entries {
        if matches!(
            lunco_storage::entry_kind_file_sync(&path),
            Ok(lunco_storage::StorageEntryKind::Directory)
        ) {
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

/// Solver options are translated once from the registered capability and params.
#[test]
fn sim_options_are_built_only_by_the_canonical_builders() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_roots = [
        root.join("src"),
        root.join("../lunco-modelica-execution/src"),
        root.join("../lunco-modelica-runner/src"),
        root.join("../lunco-modelica-solver/src"),
    ];

    let mut offenders = Vec::new();
    for source_root in source_roots {
        for file in rust_files(&source_root) {
            if file.ends_with("solver_backends.rs") {
                continue;
            }
            let Ok(text) = lunco_storage::read_text_file_sync(&file) else {
                continue;
            };
            for (i, line) in code_only(&text).lines().enumerate() {
                if line.contains("SimOptions::default()") || line.contains("SimOptions {") {
                    offenders.push(format!("{}:{}: {}", file.display(), i + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "SimOptions is built only by `lunco_modelica_solver::solver_backends::rumoca_options`, reached \
         through `stepper_options_from_bounds` (batch) or `live_stepper_options` (live); see \
         docs/architecture/29-rumoca-runtime-contracts.md.\n\
         Offending sites:\n  {}",
        offenders.join("\n  ")
    );
}

/// Authored edits preserve unclaimed source bytes through splices.
#[test]
fn source_is_never_regenerated_through_the_rumoca_emitter() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_roots = [
        root.join("src"),
        root.join("../lunco-modelica-execution/src"),
        root.join("../lunco-modelica-core/src"),
    ];

    let mut offenders = Vec::new();
    for source_root in source_roots {
        for file in rust_files(&source_root) {
            let Ok(text) = lunco_storage::read_text_file_sync(&file) else {
                continue;
            };
            for (i, line) in code_only(&text).lines().enumerate() {
                if line.contains(".to_modelica(") {
                    offenders.push(format!("{}:{}: {}", file.display(), i + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "source edits must use the shared span-splice owner; offending sites:\n {}",
        offenders.join("\n  ")
    );
}

/// User overlays enter through one owner so source capture and eviction agree.
#[test]
fn user_source_is_seated_through_one_owner() {
    let compiler_lib =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../lunco-modelica-compiler/src/lib.rs");
    let text = lunco_storage::read_text_file_sync(&compiler_lib).expect("compiler lib.rs readable");

    let seats: Vec<String> = code_only(&text)
        .lines()
        .enumerate()
        .filter(|(_, l)| {
            l.contains("session.update_document(") || l.contains("session.add_document(")
        })
        .map(|(i, l)| format!("{}:{}: {}", compiler_lib.display(), i + 1, l.trim()))
        .collect();

    // Exactly one compile-session write is expected: `seat_user_source`, the
    // chokepoint for a user model document. Library roots are parsed and
    // installed through `replace_in_memory_source_set` in
    // `load_source_root_in_memory`, so they do not add a second
    // `update_document`/`add_document` site.
    assert_eq!(
        seats.len(),
        1,
        "user overlays must enter through seat_user_source; sites: {}",
        seats.join("\n  ")
    );
}
