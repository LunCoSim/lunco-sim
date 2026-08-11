//! Reading a curriculum out of USD.
//!
//! A curriculum is a USD layer, not a bespoke manifest: a track is a prim
//! applying `LunCoTutorialTrackAPI`, and each child applying `LunCoTutorialAPI`
//! is a lesson whose script is `LunCoProgramAPI`'s `info:sourceAsset` and whose
//! world is a `payload` arc. See `assets/tutorials/sandbox/curriculum.usda`.
//!
//! WHY THE STAGE AND NOT A PARSE. Composition is the feature being bought. An
//! app offers tracks by sublayering them (`assets/tutorials/sandbox.usda`), so
//! "which tracks does this app show" and "in what order" are answered by the
//! layer stack rather than by `hosts`/`order` attributes that could disagree
//! with it; a twin contributes by composing its own layer and withdraws by
//! being removed. Re-parsing layers by hand would mean reimplementing that.
//!
//! PAYLOADS ARE NEVER LOADED HERE. The USD composition service supplies a stage
//! whose payload arcs are declarations; this module only reads those arcs, so a
//! curriculum never becomes the simulation world. Mounting stays `LoadScene`.

use bevy::prelude::*;
use openusd::{sdf, usd};

/// Pedagogical contract authored by the lesson.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LessonFormat {
    Tour,
    Exercise,
}

impl LessonFormat {
    fn read(prim: &usd::Prim, path: &str, failures: &mut Vec<String>) -> Option<Self> {
        match text(prim, "lunco:tutorial:format").as_deref() {
            Some("tour") => Some(Self::Tour),
            Some("exercise") => Some(Self::Exercise),
            Some(other) => {
                failures.push(format!(
                    "lesson '{path}' has unknown tutorial format '{other}'"
                ));
                None
            }
            None => Some(Self::Exercise),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Tour => "tour",
            Self::Exercise => "exercise",
        }
    }
}

/// One lesson, as composed.
#[derive(Clone, Debug)]
pub struct Lesson {
    /// Prim path — the lesson's IDENTITY. Progress, `StartTutorial` and the
    /// chain all key off it, so there is no separate id string to keep in step
    /// with the prim it names.
    pub path: String,
    /// Track prim path this lesson belongs to (its parent).
    pub track: String,
    pub title: String,
    pub blurb: String,
    pub difficulty: String,
    pub format: LessonFormat,
    /// `info:sourceAsset` — the `.rhai`, as an asset path (`lunco://…`, `twin://…`).
    pub script: String,
    /// The `payload` arc's asset path, or `None` when the lesson DECLARES it has
    /// no world (a UI tour). Absent is a statement here, not a missing value —
    /// that distinction is the reason the curriculum moved into USD.
    pub world: Option<String>,
    pub first_start: bool,
    /// `rel lunco:tutorial:next` — the successor's prim path.
    pub next: Option<String>,
}

/// One track's presentation.
#[derive(Clone, Debug)]
pub struct Track {
    pub path: String,
    pub label: String,
}

/// Everything one curriculum layer contributes.
#[derive(Clone, Debug, Default)]
pub struct Curriculum {
    pub tracks: Vec<Track>,
    pub lessons: Vec<Lesson>,
    /// User-actionable failures met while composing — an unopenable layer, a
    /// composition error, a lesson with no script. Each cost content the author
    /// expected to see, so the CALLER surfaces them (as `TUTORIAL_FAILED`
    /// telemetry) instead of leaving them in the log alone. The `warn!` at each
    /// site keeps the detail; this list is what reaches the status bar.
    pub failures: Vec<String>,
}

/// A `string`/`token`/`asset` attribute as text.
///
/// One reader for all three because USD distinguishes them and this code must
/// not: `sdf::Value::String`, `::Token` and `::AssetPath` are different variants
/// carrying the same characters, and matching only the one an author happened to
/// use is how an attribute silently reads back empty.
fn text(prim: &usd::Prim, name: &str) -> Option<String> {
    match prim.attribute(name).get::<sdf::Value>().ok().flatten()? {
        sdf::Value::String(s) => Some(s),
        sdf::Value::Token(t) => Some(t.to_string()),
        sdf::Value::AssetPath(a) => Some(a.to_string()),
        _ => None,
    }
}

fn flag(prim: &usd::Prim, name: &str) -> bool {
    matches!(
        prim.attribute(name).get::<sdf::Value>().ok().flatten(),
        Some(sdf::Value::Bool(true))
    )
}

/// The world a lesson declares, or `None` when it declares none.
///
/// `payload_asset_paths` READS the arc; the stage was opened with payloads
/// unloaded precisely so that asking stays a read. Strongest arc wins — a lesson
/// declares one world.
fn payload_assets(prim: &usd::Prim) -> Vec<String> {
    prim.payload_asset_paths()
        .ok()
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.to_string())
        .collect()
}

/// Project tutorial metadata from an already-composed USD stage.
///
/// Assembly belongs to `lunco-usd`; this function deliberately has no path,
/// resolver, asset reader, or layer-opening responsibility.
pub fn project(stage: &usd::Stage) -> Curriculum {
    let mut out = Curriculum::default();
    for err in stage.composition_errors() {
        warn!("[tutorial] composed curriculum stage: {err:?}");
        out.failures
            .push(format!("composed curriculum stage: {err:?}"));
    }
    let root = stage.prim(sdf::Path::abs_root());
    let Ok(top) = root.children() else {
        return out;
    };
    for track_prim in top {
        if !track_prim
            .has_api_schema("LunCoTutorialTrackAPI")
            .unwrap_or(false)
        {
            continue;
        }
        let track_path = track_prim.path().to_string();
        out.tracks.push(Track {
            path: track_path.clone(),
            label: text(&track_prim, "lunco:track:label").unwrap_or_default(),
        });

        let Ok(children) = track_prim.children() else {
            continue;
        };
        for prim in children {
            if !prim.has_api_schema("LunCoTutorialAPI").unwrap_or(false) {
                continue;
            }
            let path = prim.path().to_string();
            // The script is the one property a lesson cannot do without: with no
            // program there is nothing to run, so the lesson is not registered
            // rather than offered and then failing when a student picks it.
            let Some(script) = text(&prim, "info:sourceAsset") else {
                warn!("[tutorial] lesson '{path}' declares no info:sourceAsset — skipped");
                out.failures.push(format!(
                    "lesson '{path}' declares no info:sourceAsset — skipped"
                ));
                continue;
            };
            let payloads = payload_assets(&prim);
            if payloads.len() > 1 {
                out.failures.push(format!(
                    "lesson '{path}' declares {} payload worlds; exactly one is allowed",
                    payloads.len()
                ));
            }
            let Some(format) = LessonFormat::read(&prim, &path, &mut out.failures) else {
                continue;
            };
            out.lessons.push(Lesson {
                world: payloads.into_iter().next(),
                next: prim
                    .relationship("lunco:tutorial:next")
                    .targets()
                    .ok()
                    .and_then(|t| t.first().map(|p| p.to_string())),
                title: text(&prim, "lunco:tutorial:title").unwrap_or_else(|| path.clone()),
                blurb: text(&prim, "lunco:tutorial:blurb").unwrap_or_default(),
                difficulty: text(&prim, "lunco:tutorial:difficulty").unwrap_or_default(),
                format,
                first_start: flag(&prim, "lunco:tutorial:firstStart"),
                track: track_path.clone(),
                path,
                script,
            });
        }
    }

    // A composed curriculum is a data graph. Reject dangling successors,
    // duplicate entry points, and cycles here so the launcher never silently
    // selects the first payload or loops forever during auto-advance.
    let known: std::collections::HashSet<&str> = out
        .lessons
        .iter()
        .map(|lesson| lesson.path.as_str())
        .collect();
    for lesson in &out.lessons {
        if let Some(next) = &lesson.next {
            if !known.contains(next.as_str()) {
                out.failures.push(format!(
                    "lesson '{}' points to missing successor '{}'",
                    lesson.path, next
                ));
            }
        }
    }
    let starts: Vec<&str> = out
        .lessons
        .iter()
        .filter(|lesson| lesson.first_start)
        .map(|lesson| lesson.path.as_str())
        .collect();
    if starts.len() > 1 {
        out.failures.push(format!(
            "curriculum declares multiple firstStart lessons: {}",
            starts.join(", ")
        ));
    }
    for start in starts {
        let mut seen = std::collections::HashSet::new();
        let mut current = Some(start);
        while let Some(path) = current {
            if !seen.insert(path) {
                out.failures
                    .push(format!("curriculum successor cycle includes '{path}'"));
                break;
            }
            current = out
                .lessons
                .iter()
                .find(|lesson| lesson.path == path)
                .and_then(|lesson| lesson.next.as_deref());
        }
    }
    out
}
