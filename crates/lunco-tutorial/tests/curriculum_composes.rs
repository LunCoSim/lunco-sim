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

/// App curriculum roots are the top-level USD layers, not a list maintained by
/// this test. Adding an app root automatically brings it into these checks.
fn curriculum_apps() -> Vec<String> {
    let mut apps = std::fs::read_dir(repo("assets/tutorials"))
        .expect("tutorial asset root")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_file() {
                return None;
            }
            let path = entry.path();
            (path.extension().and_then(|ext| ext.to_str()) == Some("usda"))
                .then(|| path.file_stem()?.to_str().map(str::to_owned))
                .flatten()
        })
        .collect::<Vec<_>>();
    apps.sort();
    assert!(!apps.is_empty(), "no tutorial app roots found");
    apps
}

fn compose_app(app: &str) -> curriculum::Curriculum {
    let stage =
        lunco_usd_compose::compose_file_to_stage(&repo(&format!("assets/tutorials/{app}.usda")))
            .unwrap_or_else(|error| panic!("compose {app} curriculum: {error}"));
    curriculum::project(&stage)
}

fn tutorial_test_scenes() -> Vec<(std::path::PathBuf, String)> {
    let mut scenes = std::fs::read_dir(repo("assets/scenes/tests"))
        .expect("tutorial test scene root")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            (name.starts_with("tutorial_") && path.extension()?.to_str()? == "usda")
                .then(|| Some((path.clone(), std::fs::read_to_string(path).ok()?)))
                .flatten()
        })
        .collect::<Vec<_>>();
    scenes.sort_by(|a, b| a.0.cmp(&b.0));
    scenes
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
    assert!(
        c.failures.is_empty(),
        "luncosim curriculum failures: {:?}",
        c.failures
    );
    let labels: Vec<&str> = c.tracks.iter().map(|t| t.label.as_str()).collect();
    assert!(
        !c.tracks.is_empty(),
        "luncosim offers no tracks: {labels:?}"
    );
    assert!(!c.lessons.is_empty(), "no lessons composed");
    for t in &c.tracks {
        assert!(!t.label.is_empty(), "track {} has no label", t.path);
    }
}

#[test]
fn luncosim_offers_the_workbench_navigation_tutorial() {
    let c = compose_app("luncosim");
    let lesson = c
        .lessons
        .iter()
        .find(|lesson| lesson.path == "/Perspectives/Overview")
        .expect("luncosim tutorial catalog includes the perspective tour");
    assert_eq!(lesson.title, "View, Build & Lunica");
    assert_eq!(lesson.format, curriculum::LessonFormat::Tour);
}

/// The retired Object Builder curriculum must not reappear through an app
/// layer. Its source assets may remain available for lower-level authoring
/// work, but no shipped app may compose its track or lesson.
#[test]
fn app_curricula_do_not_offer_object_builder_lessons() {
    for app in curriculum_apps() {
        let c = compose_app(&app);
        assert!(
            c.tracks.iter().all(|track| track.label != "Object Builder"),
            "{app} still offers the Object Builder track"
        );
        assert!(
            c.lessons
                .iter()
                .all(|lesson| !lesson.path.starts_with("/ObjectBuilder/")),
            "{app} still offers an Object Builder lesson"
        );
    }
}

/// Every lesson names a script that exists. A lesson whose program is missing is
/// worse than absent: it appears in the menu and fails when a student picks it.
#[test]
fn every_lesson_resolves_its_script() {
    for app in curriculum_apps() {
        let c = compose_app(&app);
        assert!(
            c.failures.is_empty(),
            "{app} curriculum failures: {:?}",
            c.failures
        );
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
    for app in curriculum_apps() {
        let c = compose_app(&app);
        assert!(
            c.failures.is_empty(),
            "{app} curriculum failures: {:?}",
            c.failures
        );
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
    for app in curriculum_apps() {
        let c = compose_app(&app);
        assert!(
            c.failures.is_empty(),
            "{app} curriculum failures: {:?}",
            c.failures
        );
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

/// A tour may finish when the learner advances through its reference cards; an
/// exercise may not. Exercises complete only after the runtime emits the
/// objective verdict, so a content edit cannot quietly turn a simulator test
/// into a Next-button slideshow.
#[test]
fn exercises_cannot_complete_from_tour_navigation() {
    let mut tours = 0;
    let mut exercises = 0;
    for app in curriculum_apps() {
        let c = compose_app(&app);
        assert!(
            c.failures.is_empty(),
            "{app} curriculum failures: {:?}",
            c.failures
        );
        for lesson in &c.lessons {
            match lesson.format {
                curriculum::LessonFormat::Tour => tours += 1,
                curriculum::LessonFormat::Exercise => {
                    exercises += 1;
                    let path = resolve(&lesson.script).expect("bundled tutorial script");
                    let script = std::fs::read_to_string(&path)
                        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
                    assert!(
                        !script.contains("cmd:TutorialNext"),
                        "exercise {} completes through tour navigation",
                        lesson.path
                    );
                    assert!(
                        script.contains("MISSION_COMPLETE"),
                        "exercise {} has no runtime completion verdict",
                        lesson.path
                    );
                }
            }
        }
    }
    assert!(tours > 0 && exercises > 0, "expected both lesson formats");
}

/// Every shipped exercise has a production scene gate. The scene itself names
/// the lesson source, so this stays data-driven and cannot drift into a list of
/// lesson names maintained in Rust.
#[test]
fn every_exercise_has_a_production_rhai_gate() {
    let scenes = tutorial_test_scenes();
    assert!(
        !scenes.is_empty(),
        "no tutorial production test scenes found"
    );

    for app in curriculum_apps() {
        let c = compose_app(&app);
        for lesson in c
            .lessons
            .iter()
            .filter(|lesson| lesson.format == curriculum::LessonFormat::Exercise)
        {
            let needle = format!("@{}@", lesson.script);
            let Some((scene, source)) = scenes.iter().find(|(_, source)| source.contains(&needle))
            else {
                panic!(
                    "exercise {} has no scene test naming {}",
                    lesson.path, lesson.script
                );
            };
            assert!(
                source.contains("@lunco://scenarios/tests/"),
                "{} has no authored Rhai observer",
                scene.display()
            );
        }
    }
}
