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
        let value = match text(prim, "lunco:tutorial:format") {
            Ok(value) => value,
            Err(error) => {
                warn!("[tutorial] lesson '{path}' has unreadable format: {error}");
                failures.push(format!("lesson '{path}' has unreadable format: {error}"));
                return None;
            }
        };
        match value.as_deref() {
            Some("tour") => Some(Self::Tour),
            Some("exercise") => Some(Self::Exercise),
            Some(other) => {
                failures.push(format!(
                    "lesson '{path}' has unknown tutorial format '{other}'"
                ));
                None
            }
            // USD schema default: `lunco:tutorial:format` is authored as
            // `exercise` when omitted. This is the schema's authored meaning.
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
    /// Optional workbench perspective required by this track. The host applies
    /// it generically when a lesson starts; an omitted value keeps the host's
    /// normal presentation.
    pub perspective: Option<String>,
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
fn text(prim: &usd::Prim, name: &str) -> Result<Option<String>, String> {
    let value = prim
        .attribute(name)
        .get::<sdf::Value>()
        .map_err(|error| format!("attribute '{name}' could not be read: {error:?}"))?;
    match value {
        None => Ok(None),
        Some(sdf::Value::String(s)) => Ok(Some(s)),
        Some(sdf::Value::Token(t)) => Ok(Some(t.to_string())),
        Some(sdf::Value::AssetPath(a)) => Ok(Some(a.to_string())),
        Some(other) => Err(format!(
            "attribute '{name}' has unsupported value {other:?}"
        )),
    }
}

fn flag(prim: &usd::Prim, name: &str) -> Result<bool, String> {
    let value = prim
        .attribute(name)
        .get::<sdf::Value>()
        .map_err(|error| format!("attribute '{name}' could not be read: {error:?}"))?;
    match value {
        // USD schema default: `lunco:tutorial:firstStart` is false when it is
        // omitted. An authored true/false value is always preserved above.
        None => Ok(false),
        Some(sdf::Value::Bool(value)) => Ok(value),
        Some(other) => Err(format!(
            "attribute '{name}' has unsupported value {other:?}"
        )),
    }
}

/// The world a lesson declares, or `None` when it declares none.
///
/// `payload_asset_paths` READS the arc; the stage was opened with payloads
/// unloaded precisely so that asking stays a read. Strongest arc wins — a lesson
/// declares one world.
fn payload_assets(prim: &usd::Prim) -> Result<Vec<String>, String> {
    prim.payload_asset_paths()
        .map(|paths| paths.into_iter().map(|path| path.to_string()).collect())
        .map_err(|error| format!("payload arc could not be read: {error:?}"))
}

fn next_target(prim: &usd::Prim) -> Result<Option<String>, String> {
    let targets = prim
        .relationship("lunco:tutorial:next")
        .targets()
        .map_err(|error| format!("successor relationship could not be read: {error:?}"))?;
    match targets.as_slice() {
        [] => Ok(None),
        [target] => Ok(Some(target.to_string())),
        _ => Err(format!(
            "successor relationship declares {} targets; exactly one is allowed",
            targets.len()
        )),
    }
}

/// Project tutorial metadata from an already-composed USD stage.
///
/// Assembly belongs to `lunco-usd`; this function deliberately has no path,
/// resolver, asset reader, or layer-opening responsibility.
pub fn project(stage: &usd::Stage) -> Curriculum {
    let mut out = Curriculum::default();
    let program_view = lunco_usd_bevy::StageView::new(stage);
    for err in stage.composition_errors() {
        warn!("[tutorial] composed curriculum stage: {err:?}");
        out.failures
            .push(format!("composed curriculum stage: {err:?}"));
    }
    let root = stage.prim(sdf::Path::abs_root());
    let top = match root.children() {
        Ok(top) => top,
        Err(error) => {
            let detail = format!("curriculum root children could not be read: {error:?}");
            warn!("[tutorial] {detail}");
            out.failures.push(detail);
            return out;
        }
    };
    for track_prim in top {
        let is_track = match track_prim.has_api_schema("LunCoTutorialTrackAPI") {
            Ok(value) => value,
            Err(error) => {
                let detail = format!(
                    "track '{}' schema could not be read: {error:?}",
                    track_prim.path()
                );
                warn!("[tutorial] {detail}");
                out.failures.push(detail);
                continue;
            }
        };
        if !is_track {
            continue;
        }
        let track_path = track_prim.path().to_string();
        let label = match text(&track_prim, "lunco:track:label") {
            Ok(Some(value)) if !value.trim().is_empty() => value,
            Ok(_) => {
                let detail = format!("track '{track_path}' declares no non-empty label");
                warn!("[tutorial] {detail}");
                out.failures.push(detail);
                continue;
            }
            Err(error) => {
                let detail = format!("track '{track_path}' has unreadable label: {error}");
                warn!("[tutorial] {detail}");
                out.failures.push(detail);
                continue;
            }
        };
        let perspective = match text(&track_prim, "lunco:track:perspective") {
            Ok(value) => value.filter(|value| !value.trim().is_empty()),
            Err(error) => {
                let detail = format!("track '{track_path}' has unreadable perspective: {error}");
                warn!("[tutorial] {detail}");
                out.failures.push(detail);
                continue;
            }
        };
        let children = match track_prim.children() {
            Ok(children) => children,
            Err(error) => {
                let detail = format!("track '{track_path}' children could not be read: {error:?}");
                warn!("[tutorial] {detail}");
                out.failures.push(detail);
                continue;
            }
        };
        out.tracks.push(Track {
            path: track_path.clone(),
            label,
            perspective,
        });
        for prim in children {
            let is_lesson = match prim.has_api_schema("LunCoTutorialAPI") {
                Ok(value) => value,
                Err(error) => {
                    let detail = format!(
                        "lesson candidate '{}' schema could not be read: {error:?}",
                        prim.path()
                    );
                    warn!("[tutorial] {detail}");
                    out.failures.push(detail);
                    continue;
                }
            };
            if !is_lesson {
                continue;
            }
            let path = prim.path().to_string();
            // The script is the one property a lesson cannot do without: with no
            // program there is nothing to run, so the lesson is not registered
            // rather than offered and then failing when a student picks it.
            let script = match lunco_usd_bevy::program::resolve_program(&program_view, &prim.path())
            {
                Ok(lunco_usd_bevy::program::ResolvedProgram {
                    backend: lunco_usd_bevy::program::ProgramBackend::Rhai,
                    source: lunco_usd_bevy::program::ProgramSource::Asset(source),
                }) => source,
                Ok(_) => {
                    let detail =
                        format!("lesson '{path}' must select a Rhai info:sourceAsset; skipped");
                    warn!("[tutorial] {detail}");
                    out.failures.push(detail);
                    continue;
                }
                Err(issue) => {
                    let detail = format!(
                        "lesson '{path}' has unresolved program {}: {}",
                        issue.property, issue.message
                    );
                    warn!("[tutorial] {detail}");
                    out.failures.push(detail);
                    continue;
                }
            };
            let payloads = match payload_assets(&prim) {
                Ok(payloads) => payloads,
                Err(error) => {
                    let detail = format!("lesson '{path}' has unreadable payload: {error}");
                    warn!("[tutorial] {detail}");
                    out.failures.push(detail);
                    continue;
                }
            };
            if payloads.len() > 1 {
                let detail = format!(
                    "lesson '{path}' declares {} payload worlds; exactly one is allowed",
                    payloads.len()
                );
                warn!("[tutorial] {detail}");
                out.failures.push(detail);
                continue;
            }
            let Some(format) = LessonFormat::read(&prim, &path, &mut out.failures) else {
                continue;
            };
            let next = match next_target(&prim) {
                Ok(next) => next,
                Err(error) => {
                    let detail = format!("lesson '{path}' has unreadable successor: {error}");
                    warn!("[tutorial] {detail}");
                    out.failures.push(detail);
                    continue;
                }
            };
            let title = match text(&prim, "lunco:tutorial:title") {
                Ok(Some(value)) if !value.trim().is_empty() => value,
                Ok(_) => {
                    let detail = format!("lesson '{path}' declares no non-empty title");
                    warn!("[tutorial] {detail}");
                    out.failures.push(detail);
                    continue;
                }
                Err(error) => {
                    let detail = format!("lesson '{path}' has unreadable title: {error}");
                    warn!("[tutorial] {detail}");
                    out.failures.push(detail);
                    continue;
                }
            };
            let blurb = match text(&prim, "lunco:tutorial:blurb") {
                // USD schema default: an omitted blurb is intentionally empty.
                Ok(value) => value.unwrap_or_default(),
                Err(error) => {
                    let detail = format!("lesson '{path}' has unreadable blurb: {error}");
                    warn!("[tutorial] {detail}");
                    out.failures.push(detail);
                    continue;
                }
            };
            let difficulty = match text(&prim, "lunco:tutorial:difficulty") {
                // USD schema default: an omitted difficulty is `beginner`.
                Ok(value) => value.unwrap_or_else(|| "beginner".to_owned()),
                Err(error) => {
                    let detail = format!("lesson '{path}' has unreadable difficulty: {error}");
                    warn!("[tutorial] {detail}");
                    out.failures.push(detail);
                    continue;
                }
            };
            let first_start = match flag(&prim, "lunco:tutorial:firstStart") {
                Ok(value) => value,
                Err(error) => {
                    let detail = format!("lesson '{path}' has unreadable firstStart: {error}");
                    warn!("[tutorial] {detail}");
                    out.failures.push(detail);
                    continue;
                }
            };
            out.lessons.push(Lesson {
                world: payloads.into_iter().next(),
                next,
                title,
                blurb,
                difficulty,
                format,
                first_start,
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
