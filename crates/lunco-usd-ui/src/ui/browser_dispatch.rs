//! Routes [`lunco_workbench_browser::BrowserAction::OpenFile`] events with USD
//! extensions (`.usda`, `.usd`, `.usdc`) into the USD document open pipeline.
//!
//! A browser click means **open and preview this source**, never **replace the
//! running scene**. A Twin contains reusable vehicle, material, and support
//! layers as well as scene roots. Source inspection uses
//! [`OpenUsdSourceDocument`]; loading a world is an explicit Scenarios action.
//!
//! ## File partitioning
//!
//! [`BrowserActions::take_where`] only removes the actions whose path
//! has a `.usda` / `.usd` / `.usdc` extension, leaving Modelica's `.mo` opens
//! for the Modelica drain to handle in the same frame. Two crates,
//! one shared outbox, no ordering coupling.
//!
//! ## UI-only
//!
//! This module just translates browser-panel clicks into the document-load
//! pipeline. The filesystem read and registry allocation live in the USD
//! command owner so they also work in headless bins that never add
//! `UsdUiPlugin`.

use bevy::prelude::*;
use lunco_usd_core::commands::{OpenUsdSourceDocument, is_usd_path};
use lunco_workbench_browser::{BrowserAction, BrowserActions};
use lunco_workspace::WorkspaceResource;

fn is_usd_open_file(action: &BrowserAction) -> bool {
    match action {
        BrowserAction::OpenFile { relative_path } => is_usd_path(&relative_path.to_string_lossy()),
        _ => false,
    }
}

/// Resolve a browser file selection to an on-disk document.
///
/// Browser sections may already know an absolute path. That form is already
/// resolved by the emitting section and must not be re-anchored on the active
/// Twin; relative paths remain scoped to the active Twin.
fn browser_document_path(
    root: Option<&std::path::Path>,
    selected: &std::path::Path,
) -> Option<std::path::PathBuf> {
    if selected.is_absolute() {
        return Some(selected.to_path_buf());
    }
    let root = root?;
    let absolute = root.join(selected);
    if absolute.strip_prefix(root).is_ok() {
        Some(absolute)
    } else {
        None
    }
}

/// Drain Twin-browser `OpenFile` actions whose path looks like USD and hand
/// each off to the document pipeline through [`OpenUsdSourceDocument`].
/// This deliberately does not trigger the scene-load command.
pub fn drain_browser_actions_for_usd(world: &mut World) {
    let actions: Vec<BrowserAction> = {
        // Bail gracefully when the workbench's outbox isn't present
        // (headless / lifecycle tests add `UsdUiPlugin` without the
        // workbench plugin). `resource_mut` would panic.
        let Some(mut outbox) = world.get_resource_mut::<BrowserActions>() else {
            return;
        };
        outbox.take_where(is_usd_open_file)
    };
    if actions.is_empty() {
        return;
    }

    let active_twin = world
        .get_resource::<WorkspaceResource>()
        .and_then(|ws| ws.active_twin.and_then(|id| ws.twin(id)))
        .map(|twin| twin.root.clone());
    for action in actions {
        let BrowserAction::OpenFile { relative_path } = action else {
            continue;
        };
        let Some(abs) = browser_document_path(active_twin.as_deref(), &relative_path) else {
            bevy::log::warn!(
                "BrowserAction::OpenFile (USD) needs an active Twin for relative path: {:?}",
                relative_path
            );
            continue;
        };
        world.trigger(OpenUsdSourceDocument {
            source: abs.to_string_lossy().into_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_usd_dispatch_emits_source_intent_and_keeps_other_actions() {
        #[derive(Resource, Default)]
        struct Sources(Vec<String>);
        let mut app = App::new();
        app.init_resource::<BrowserActions>()
            .init_resource::<Sources>()
            .add_observer(
                |event: On<OpenUsdSourceDocument>, mut sources: ResMut<Sources>| {
                    sources.0.push(event.event().source.clone());
                },
            );
        for path in ["/external/assembly.usda", "/external/model.mo"] {
            app.world_mut()
                .resource_mut::<BrowserActions>()
                .push(BrowserAction::OpenFile {
                    relative_path: path.into(),
                });
        }
        drain_browser_actions_for_usd(app.world_mut());
        assert_eq!(
            app.world().resource::<Sources>().0,
            ["/external/assembly.usda"]
        );
        let remaining = app.world_mut().resource_mut::<BrowserActions>().drain();
        assert_eq!(remaining.len(), 1);
        assert!(
            matches!(&remaining[0], BrowserAction::OpenFile { relative_path }
            if relative_path == std::path::Path::new("/external/model.mo"))
        );
    }

    #[test]
    fn browser_usd_selection_stays_within_the_active_twin() {
        let root = std::path::PathBuf::from("/tmp/lunco-browser-dispatch/twin");
        let rover = root.join("sim").join("rovers").join("lunokhod2.usda");
        let traverse = root.join("sim").join("scenes").join("traverse.usda");
        let outside = root
            .parent()
            .expect("test root has a parent")
            .join("solar_system.usda");

        assert_eq!(
            browser_document_path(
                Some(&root),
                std::path::Path::new("sim/rovers/lunokhod2.usda"),
            ),
            Some(rover)
        );
        assert_eq!(
            browser_document_path(Some(&root), &traverse),
            Some(traverse)
        );
        assert_eq!(
            browser_document_path(Some(&root), &outside),
            Some(outside),
            "an already-resolved absolute browser path must not be re-anchored on the active Twin"
        );
    }

    #[test]
    fn browser_usd_filter_uses_the_domain_extension_contract() {
        for path in ["scene.usda", "scene.usd", "scene.USDC"] {
            assert!(is_usd_open_file(&BrowserAction::OpenFile {
                relative_path: std::path::PathBuf::from(path),
            }));
        }
        assert!(!is_usd_open_file(&BrowserAction::OpenFile {
            relative_path: std::path::PathBuf::from("scene.usdz"),
        }));
        assert!(!is_usd_open_file(&BrowserAction::OpenModelicaClass {
            relative_path: std::path::PathBuf::from("scene.usd"),
            qualified_path: "Scene".to_string(),
        }));
    }
}
