//! The joint-attachment rule, enforced against the source tree.
//!
//! `attach_joint` is the only way a joint may enter the world, and the type
//! system carries most of that: a builder returns [`JointSpec`], which is not a
//! `Component` and whose inner value is private, so no other crate can insert a
//! joint even by trying.
//!
//! What types cannot cover is a caller that builds an avian joint from scratch —
//! `RevoluteJoint::new(a, b)` is avian's API and always available. That is
//! precisely how the wheel joint came to bypass the admission gate, so it is
//! what this test watches for. The two rules a bypass breaks are not style:
//!
//! - a joint entering avian's graph before both bodies are in the island graph
//!   panics in `merge_islands` ("Neither body … is in an island");
//! - a jointed pair that reaches the narrow phase first leaves a freed
//!   `ContactId` in an island list when the joint disables the pair ("Contact has
//!   no island").
//!
//! Both are hard crashes on an ordinary scene switch, not degraded behaviour.

use std::path::{Path, PathBuf};

/// avian joint components. Constructing one of these outside `lunco-usd-avian`
/// means a second attachment path is being built.
const JOINT_TYPES: [&str; 5] = [
    "RevoluteJoint",
    "PrismaticJoint",
    "FixedJoint",
    "SphericalJoint",
    "DistanceJoint",
];

/// The crate that OWNS joint construction — the one place these names may appear.
const OWNER: &str = "lunco-usd-avian";

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ is the parent of this crate")
        .to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn only_lunco_usd_avian_constructs_avian_joints() {
    let root = crates_dir();
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);
    assert!(!sources.is_empty(), "found no Rust sources under {root:?}");

    let mut offenders: Vec<String> = Vec::new();
    for file in sources {
        // The owner builds joints; its own tests may name them freely.
        if file.components().any(|c| c.as_os_str() == OWNER) {
            continue;
        }
        // TEST code is exempt, and deliberately so. A test builds its own world
        // to prove one behaviour — often the plain-avian behaviour this crate's
        // rules exist to work around — and it neither loads a USD scene nor
        // survives a scene switch. The rule governs the code that builds the
        // RUNNING world.
        if file.components().any(|c| c.as_os_str() == "tests") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        // Everything from the first `#[cfg(test)]` on is that file's test module
        // (the workspace convention is one, last). Same exemption, same reason.
        let production = match text.find("#[cfg(test)]") {
            Some(at) => &text[..at],
            None => &text[..],
        };
        for (n, line) in production.lines().enumerate() {
            // Constructor calls only. A mention in a comment or a doc link is
            // not an attachment path, and this rule is about code.
            let code = line.split("//").next().unwrap_or("");
            for joint in JOINT_TYPES {
                if code.contains(&format!("{joint}::new(")) {
                    offenders.push(format!(
                        "{}:{}: {}",
                        file.strip_prefix(&root).unwrap_or(&file).display(),
                        n + 1,
                        code.trim()
                    ));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "avian joints may only be built in `{OWNER}`, and may only reach the world \
         through `attach_joint` — it is what defers the joint until both bodies are \
         in avian's island graph and filters the pair out of the narrow phase. \
         Build the joint behind a constructor in `{OWNER}` that returns \
         `JointSpec`, and call `attach_joint` with it.\n  {}",
        offenders.join("\n  ")
    );
}
