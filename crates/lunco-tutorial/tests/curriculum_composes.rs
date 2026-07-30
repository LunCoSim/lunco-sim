//! The shipped curriculum layers compose, and every lesson they declare is real.
//!
//! A curriculum is data loaded at runtime, so a typo'd asset path or a `next`
//! naming a lesson that does not exist is invisible to the compiler and shows up
//! as a lesson that opens to nothing — or a chain that strands a student after
//! the last step. These assertions are enumerated from what composes, never from
//! a hardcoded list, so a NEW track is covered the moment it exists.

use lunco_tutorial::curriculum;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Resolve an authored asset path to a file on disk. Mirrors what the launcher
/// must do: `lunco://` is engine-shipped, `twin://` belongs to a twin.
fn resolve(asset: &str) -> Option<std::path::PathBuf> {
    asset
        .strip_prefix("lunco://")
        .map(|rest| repo("assets").join(rest))
}

/// The unified LunCoSim app composes its flagship tour plus the reusable
/// sandbox-authoring and basic-driving tracks. A track is not owned by the app
/// it happens to be named after; the app root is the sole offer/order manifest.
#[test]
fn the_app_layer_composes_the_tracks_it_offers() {
    let stage = lunco_usd_compose::compose_file_to_stage(&repo("assets/tutorials/luncosim.usda"))
        .expect("compose luncosim curriculum");
    let c = curriculum::project(&stage);
    let labels: Vec<&str> = c.tracks.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(
        c.tracks.len(),
        3,
        "expected luncosim + sandbox + basic, got {labels:?}"
    );
    assert!(!c.lessons.is_empty(), "no lessons composed");
    for t in &c.tracks {
        assert!(!t.label.is_empty(), "track {} has no label", t.path);
    }
}

/// Every lesson names a script that exists. A lesson whose program is missing is
/// worse than absent: it appears in the menu and fails when a student picks it.
#[test]
fn every_lesson_resolves_its_script() {
    for app in ["luncosim", "lunica", "sandbox"] {
        let stage = lunco_usd_compose::compose_file_to_stage(&repo(&format!(
            "assets/tutorials/{app}.usda"
        )))
        .expect("compose curriculum");
        let c = curriculum::project(&stage);
        for lesson in &c.lessons {
            let path = resolve(&lesson.script).unwrap_or_else(|| {
                panic!("{}: unresolvable script '{}'", lesson.path, lesson.script)
            });
            assert!(
                path.is_file(),
                "{}: script '{}' does not exist",
                lesson.path,
                lesson.script
            );
        }
    }
}

/// A declared world must exist — and a lesson with NO world is legitimate.
///
/// That second half is why the world is DECLARED: `b5_join_team` and the whole
/// lunica track have no world on purpose, and a lesson that opened its own would
/// make "has none" indistinguishable from "forgot one".
#[test]
fn declared_worlds_exist_and_world_less_lessons_are_allowed() {
    let mut with = 0;
    let mut without = 0;
    for app in ["luncosim", "lunica", "sandbox"] {
        let stage = lunco_usd_compose::compose_file_to_stage(&repo(&format!(
            "assets/tutorials/{app}.usda"
        )))
        .expect("compose curriculum");
        let c = curriculum::project(&stage);
        for lesson in &c.lessons {
            match &lesson.world {
                Some(w) => {
                    let path = resolve(w)
                        .unwrap_or_else(|| panic!("{}: unresolvable world '{w}'", lesson.path));
                    assert!(
                        path.is_file(),
                        "{}: world '{w}' does not exist",
                        lesson.path
                    );
                    with += 1;
                }
                None => without += 1,
            }
        }
    }
    assert!(
        with > 0 && without > 0,
        "expected both kinds, got {with} with / {without} without"
    );
}

/// A chain must not strand a student: every `next` targets a composed lesson.
#[test]
fn no_lesson_chains_to_a_lesson_that_does_not_exist() {
    for app in ["luncosim", "lunica", "sandbox"] {
        let stage = lunco_usd_compose::compose_file_to_stage(&repo(&format!(
            "assets/tutorials/{app}.usda"
        )))
        .expect("compose curriculum");
        let c = curriculum::project(&stage);
        let known: std::collections::HashSet<&str> =
            c.lessons.iter().map(|l| l.path.as_str()).collect();
        for lesson in &c.lessons {
            if let Some(next) = &lesson.next {
                assert!(
                    known.contains(next.as_str()),
                    "{} chains to '{next}', which no lesson defines",
                    lesson.path
                );
            }
        }
    }
}
