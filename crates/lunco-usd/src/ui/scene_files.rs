//! **Scene Files** — every file the loaded scene is actually made of.
//!
//! # Why this is not the Files section
//!
//! [`FilesSection`](lunco_workbench::FilesSection) shows a FOLDER (the active
//! Twin's tree) and [`UsdSceneSection`](crate::ui::browser_section::UsdSceneSection)
//! shows the open STAGES. Neither answers "what is this scene composed of", which
//! is a graph question: a scene pulls its rovers from `assets/vessels/…`, those
//! pull components from `assets/components/…`, and those bind `.mo` models,
//! `.rhai` policies, shaders and textures by attribute. None of that is in the
//! scene's folder, and most of it is in no open tab.
//!
//! # The `lunco://` hole this had to close first
//!
//! Shipped assets are REQUIRED to be referenced as `@lunco://…@`, and
//! [`lunco_assets::transitive_file_closure`] drops every schemed arc because it
//! has no resolver — so the plain walk reports a library-built scene as one
//! file. This section uses [`lunco_assets::transitive_file_closure_with`] and
//! supplies the resolver: `lunco://` against the shipped asset root, `twin://`
//! against [`TwinRoots`]. USD dependency interpretation comes from
//! `lunco-usd-compose`; asset traversal and storage stay in `lunco-assets`.
//! Anything it still cannot reach is COUNTED and shown, so a partial answer
//! never reads as a complete one.
//!
//! # The Modelica schemes come for free
//!
//! A `.mo` row opens a model tab, and the diagram is projected from the class
//! whether or not the source carries `annotation(Placement(…))` — un-annotated
//! components are laid out on a grid by
//! `DiagramAutoLayoutSettings`. So "show the scene's auto-generated Modelica
//! schemes" needs no diagram work here: it needs the `.mo` files to be REACHABLE
//! and clickable, which is exactly what the resolved closure provides.
//!
//! # Cost
//!
//! The walk parses every USD layer it reaches, so it is change-gated hard: it runs
//! when the SET of scene roots changes, or when the user asks for a refresh —
//! never per frame, and never during paint (a `BrowserSection::render` gets a
//! read-only view-model, like every other section under the WP-8 contract).

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_assets::TwinRoots;
use lunco_doc::DocumentOrigin;
use lunco_doc_bevy::{DocumentRegistry, OpenFile};
use lunco_workbench::twin_browser::BrowserScope;
use lunco_workbench::{BrowserAction, BrowserCtx, BrowserSection};

use crate::document::UsdDocument;

/// What kind of file a row is — decides its group and its click action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SceneFileKind {
    /// A USD layer (`.usda`/`.usd`/`.usdc`) — the scene itself and everything it
    /// references.
    Layer,
    /// A Modelica model bound by `info:sourceAsset`. Opens as a diagram.
    Modelica,
    /// A `.rhai` scenario / policy.
    Script,
    /// A WGSL shader.
    Shader,
    /// Anything else the closure carried: meshes, textures, DEMs.
    Asset,
}

impl SceneFileKind {
    fn of(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("usda" | "usd" | "usdc" | "usdz") => Self::Layer,
            Some("mo") => Self::Modelica,
            Some("rhai") => Self::Script,
            Some("wgsl") => Self::Shader,
            _ => Self::Asset,
        }
    }

    /// Group header, in render order.
    pub fn title(self) -> &'static str {
        match self {
            Self::Layer => "USD layers",
            Self::Modelica => "Modelica models",
            Self::Script => "Scripts",
            Self::Shader => "Shaders",
            Self::Asset => "Assets",
        }
    }

    /// Every kind, in the order the section paints them: the structure first,
    /// then behaviour, then the leaves.
    pub const ORDER: [SceneFileKind; 5] = [
        Self::Layer,
        Self::Modelica,
        Self::Script,
        Self::Shader,
        Self::Asset,
    ];
}

/// One file in the scene's closure.
#[derive(Debug, Clone)]
pub struct SceneFileRow {
    /// Absolute path on disk.
    pub path: PathBuf,
    /// Display label — the path relative to its library/twin root when it has
    /// one, so rows read `vessels/rovers/rocker_bogie.usda` rather than a wall of
    /// absolute prefix.
    pub label: String,
    /// Which group it renders under.
    pub kind: SceneFileKind,
    /// The file the closure named does not exist on disk. Shown rather than
    /// hidden: a dangling reference is precisely what someone opening this
    /// section is hunting for.
    pub missing: bool,
}

/// Change-gated view-model of the loaded scene's file closure. Produced by
/// [`produce_scene_file_view`]; read (never built) by [`SceneFilesSection`].
#[derive(Resource, Default)]
pub struct SceneFileView {
    /// The scene roots the closure was walked from.
    pub roots: Vec<PathBuf>,
    /// Every reachable file, grouped by [`SceneFileKind`] at paint time.
    pub rows: Vec<SceneFileRow>,
    /// References that named a scheme this session cannot resolve (a `twin://`
    /// with no such Twin mounted). Reported as a count so a partial listing is
    /// never mistaken for a complete one.
    pub unresolved: usize,
}

/// Set by the section's ↻ button to force one rebuild — the roots did not change,
/// but the files on disk may have.
#[derive(Resource, Default)]
pub struct SceneFileRescan(pub bool);

/// Resolve a schemed reference to a file. `lunco://` re-roots on the shipped
/// asset library, `twin://` on the named Twin's root; anything else (a leading
/// `/`, an unknown scheme) is unreachable and counted by the caller.
/// `twins` is optional for the same reason the system's `Res` is: a host with no
/// Twin source mounted can still resolve the shipped library, and `twin://` there
/// is simply unreachable (counted, not fatal).
fn resolve_scheme(
    reference: &str,
    assets_root: Option<&Path>,
    twins: Option<&TwinRoots>,
) -> Option<PathBuf> {
    if let Some(rel) = lunco_assets::parse_lunco_uri(reference) {
        if !lunco_assets::asset_path::is_safe_relative_path(rel) {
            return None;
        }
        return Some(assets_root?.join(rel));
    }
    if let Some((name, rel)) = lunco_assets::parse_twin_uri(reference) {
        let relative = lunco_assets::asset_path::relative_path(rel)?;
        return twins?.resolve_path(name, &relative);
    }
    None
}

/// The shipped asset root to resolve `lunco://` against: the `assets/` ancestor
/// of a scene root when the scene lives inside a library tree, else the running
/// project's own `assets/`. Both are what the `lunco://` asset SOURCE would use,
/// which is the point — the browser must list the files the engine would load.
fn assets_root_for(roots: &[PathBuf]) -> Option<PathBuf> {
    for r in roots {
        if let Some(found) = lunco_assets::shipped_asset_root(r) {
            return Some(found.to_path_buf());
        }
    }
    let cwd = lunco_assets::assets_dir_abs();
    cwd.is_dir().then_some(cwd)
}

/// Label a path relative to whichever root it sits under, so rows stay readable.
fn label_for(path: &Path, assets_root: Option<&Path>, roots: &[PathBuf]) -> String {
    if let Some(rel) = assets_root.and_then(|a| path.strip_prefix(a).ok()) {
        return rel.to_string_lossy().into_owned();
    }
    for r in roots {
        if let Some(dir) = r.parent() {
            if let Ok(rel) = path.strip_prefix(dir) {
                return rel.to_string_lossy().into_owned();
            }
        }
    }
    path.to_string_lossy().into_owned()
}

/// Producer for [`SceneFileView`]. Walks the resolved reference closure of every
/// open file-backed USD document.
///
/// Gated on the ROOT SET (plus an explicit rescan request): the walk parses every
/// layer it reaches, which is filesystem work that must not ride the frame.
pub fn produce_scene_file_view(
    registry: Option<Res<DocumentRegistry<UsdDocument>>>,
    // OPTIONAL, both of them. This plugin is added by panel-level tests and by
    // hosts that install no asset sources at all; a hard `Res` there is not a
    // missing feature but a PANIC in `Main`, taking the whole app down to
    // populate a browser list. Absent registry ⇒ no scene roots ⇒ nothing to
    // walk; absent `TwinRoots` ⇒ `twin://` arcs are simply unresolvable, which
    // the view already reports as `unresolved`.
    twins: Option<Res<TwinRoots>>,
    mut view: ResMut<SceneFileView>,
    mut rescan: ResMut<SceneFileRescan>,
    mut last_roots: Local<Vec<PathBuf>>,
) {
    let Some(registry) = registry else {
        return;
    };
    let mut roots: Vec<PathBuf> = registry
        .ids()
        .filter_map(|id| registry.host(id))
        .filter_map(|h| match h.document().origin() {
            DocumentOrigin::File { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();
    roots.sort();
    roots.dedup();

    let forced = std::mem::replace(&mut rescan.0, false);
    if !forced && *last_roots == roots {
        return;
    }
    last_roots.clone_from(&roots);

    let assets_root = assets_root_for(&roots);
    let unresolved = std::sync::atomic::AtomicUsize::new(0);
    let files = lunco_assets::transitive_file_closure_with(
        &roots,
        |reference| {
            let resolved = resolve_scheme(reference, assets_root.as_deref(), twins.as_deref());
            if resolved.is_none() {
                unresolved.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            resolved
        },
        lunco_usd_compose::is_usd_layer,
        lunco_usd_compose::layer_dependency_arcs,
    );

    let mut rows: Vec<SceneFileRow> = files
        .into_iter()
        .map(|path| SceneFileRow {
            label: label_for(&path, assets_root.as_deref(), &roots),
            kind: SceneFileKind::of(&path),
            missing: !path.exists(),
            path,
        })
        .collect();
    rows.sort_by(|a, b| (a.kind, &a.label).cmp(&(b.kind, &b.label)));

    view.roots = roots;
    view.rows = rows;
    view.unresolved = unresolved.into_inner();
}

/// Browser section listing the loaded scene's file closure.
///
/// Files scope: it answers "what is on disk behind this scene", the same question
/// the Files tree answers for a folder. The Models tab keeps the typed views
/// (stages, Modelica classes).
#[derive(Default)]
pub struct SceneFilesSection;

impl BrowserSection for SceneFilesSection {
    fn id(&self) -> &str {
        "lunco.usd.scene-files"
    }

    fn title(&self) -> &str {
        "Scene"
    }

    fn scope(&self) -> BrowserScope {
        BrowserScope::Files
    }

    fn default_open(&self) -> bool {
        true
    }

    fn order(&self) -> u32 {
        // Above the raw folder tree (200): what the scene IS comes before what
        // happens to sit next to it on disk.
        150
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_, '_>) {
        let Some(view) = ctx.resource::<SceneFileView>() else {
            ui.label(
                egui::RichText::new("SceneFileView resource missing")
                    .weak()
                    .italics(),
            );
            return;
        };
        // Snapshot before any `&mut` dispatch below.
        let rows = view.rows.clone();
        let unresolved = view.unresolved;
        let no_roots = view.roots.is_empty();

        if ui
            .small_button("↻")
            .on_hover_text("Re-walk the scene's references")
            .clicked()
        {
            ctx.set_resource(SceneFileRescan(true));
        }

        if no_roots {
            ui.label(
                egui::RichText::new("No file-backed scene open.")
                    .weak()
                    .italics(),
            );
            return;
        }

        let mut clicked: Option<(PathBuf, SceneFileKind)> = None;
        for kind in SceneFileKind::ORDER {
            let group: Vec<&SceneFileRow> = rows.iter().filter(|r| r.kind == kind).collect();
            if group.is_empty() {
                continue;
            }
            egui::CollapsingHeader::new(format!("{} ({})", kind.title(), group.len()))
                .id_salt(("scene_files_group", kind.title()))
                .default_open(matches!(
                    kind,
                    SceneFileKind::Layer | SceneFileKind::Modelica
                ))
                .show(ui, |ui| {
                    for row in group {
                        let label = if row.missing {
                            format!("⚠ {}", row.label)
                        } else {
                            row.label.clone()
                        };
                        // Openable rows are routed through the normal typed
                        // `OpenFile` surface. USD and Modelica have richer
                        // editors; WGSL intentionally opens in the shared
                        // read-only source viewer so the exact shader driving
                        // the scene is inspectable from the scene closure.
                        let openable = !row.missing
                            && matches!(
                                row.kind,
                                SceneFileKind::Layer
                                    | SceneFileKind::Modelica
                                    | SceneFileKind::Shader
                            );
                        let resp = if openable {
                            ui.selectable_label(false, label)
                        } else {
                            // `sense(hover)` so the tooltip below still fires —
                            // a bare `Label` senses nothing and would swallow it.
                            ui.add(
                                egui::Label::new(egui::RichText::new(label).weak())
                                    .sense(egui::Sense::hover()),
                            )
                        };
                        let resp = if row.missing {
                            resp.on_hover_text("Referenced by the scene but not on disk")
                        } else if openable {
                            resp.on_hover_text(row.path.to_string_lossy())
                        } else {
                            resp.on_hover_text(format!(
                                "{} — part of the scene; no editor is bound to this type",
                                row.path.to_string_lossy()
                            ))
                        };
                        if openable && resp.clicked() {
                            clicked = Some((row.path.clone(), row.kind));
                        }
                    }
                });
        }

        if unresolved > 0 {
            ui.label(
                egui::RichText::new(format!(
                    "{unresolved} reference(s) could not be resolved (unmounted Twin or unknown \
                     scheme) — the list is partial."
                ))
                .small()
                .weak(),
            );
        }

        if let Some((path, kind)) = clicked {
            if kind == SceneFileKind::Shader {
                // A shader is source text, not a USD/Modelica document. Send the
                // typed source command directly so the shared text viewer owns
                // the absolute scene-closure path; BrowserAction is intentionally
                // reserved for domain document dispatchers.
                ctx.trigger(OpenFile {
                    path: path.to_string_lossy().into_owned(),
                });
            } else {
                // Absolute path: the domain dispatchers (`.mo` → Modelica model
                // tab, `.usda` → USD) take it as-is rather than anchoring on the
                // active Twin, because a scene's files routinely live outside it.
                ctx.actions.push(BrowserAction::OpenFile {
                    relative_path: path,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_route_by_extension() {
        assert_eq!(
            SceneFileKind::of(Path::new("/a/scene.usda")),
            SceneFileKind::Layer
        );
        assert_eq!(
            SceneFileKind::of(Path::new("/a/Drive.mo")),
            SceneFileKind::Modelica
        );
        assert_eq!(
            SceneFileKind::of(Path::new("/a/patrol.rhai")),
            SceneFileKind::Script
        );
        assert_eq!(
            SceneFileKind::of(Path::new("/a/wheel.wgsl")),
            SceneFileKind::Shader
        );
        assert_eq!(
            SceneFileKind::of(Path::new("/a/rover.glb")),
            SceneFileKind::Asset
        );
    }

    #[test]
    fn lunco_references_resolve_against_the_library_root() {
        let twins = TwinRoots::default();
        let assets = PathBuf::from("/proj/assets");
        assert_eq!(
            resolve_scheme("lunco://vessels/rover.usda", Some(&assets), Some(&twins)),
            Some(assets.join("vessels/rover.usda"))
        );
        // The shipped library does not need a Twin source to be mounted.
        assert_eq!(
            resolve_scheme("lunco://vessels/rover.usda", Some(&assets), None),
            Some(assets.join("vessels/rover.usda"))
        );
        assert_eq!(
            resolve_scheme("twin://nope/scene.usda", Some(&assets), Some(&twins)),
            None,
            "an unmounted twin is unreachable, not silently mis-rooted"
        );
        assert_eq!(
            resolve_scheme(
                "/absolute/from/source/root.usda",
                Some(&assets),
                Some(&twins)
            ),
            None
        );
    }

    #[test]
    fn twin_references_resolve_against_the_mounted_root() {
        let twins = TwinRoots::default();
        let name = twins.register("moonbase", "/twins/moonbase");
        let uri = format!("twin://{name}/scenes/base.usda");
        assert_eq!(
            resolve_scheme(&uri, None, Some(&twins)),
            Some(PathBuf::from("/twins/moonbase/scenes/base.usda"))
        );
    }

    #[test]
    fn scheme_references_cannot_escape_their_root() {
        let twins = TwinRoots::default();
        let assets = PathBuf::from("/proj/assets");
        let name = twins.register("moonbase", "/twins/moonbase");
        for reference in [
            "lunco://../outside.usda",
            "lunco://vessels/../../outside.usda",
            &format!("twin://{name}/../outside.usda"),
            &format!("twin://{name}/scenes/../../outside.usda"),
        ] {
            assert_eq!(
                resolve_scheme(reference, Some(&assets), Some(&twins)),
                None,
                "unsafe reference must be rejected: {reference}"
            );
        }
    }

    #[test]
    fn labels_are_relative_to_the_library_root() {
        let assets = PathBuf::from("/proj/assets");
        assert_eq!(
            label_for(
                Path::new("/proj/assets/vessels/rovers/rb.usda"),
                Some(&assets),
                &[]
            ),
            "vessels/rovers/rb.usda"
        );
        // Outside the library: relative to the scene's own folder.
        assert_eq!(
            label_for(
                Path::new("/work/scenes/props/rock.usda"),
                Some(&assets),
                &[PathBuf::from("/work/scenes/scene.usda")]
            ),
            "props/rock.usda"
        );
    }
}
