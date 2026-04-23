//! Modelica scheme handler for [`UriRegistry`](lunco_workbench::UriRegistry).
//!
//! Handles both flavours of `modelica://` URI defined in MLS Annex D:
//!
//! * **Class references** — `modelica://Modelica.Blocks.Examples.PID`,
//!   optionally with an anchor: `...#Overview`. Resolved to
//!   [`UriResolution::OpenDocument`] (or `NavigateAnchor` when
//!   `#anchor` is present) with `doc_kind = "modelica"`.
//!
//! * **Resource references** — `modelica://Modelica.Blocks/Resources/
//!   Images/foo.png`. The portion up to the first `/` is the class
//!   path (dotted), the remainder is a subpath inside that class's
//!   package directory on disk. Resolved to
//!   [`UriResolution::OpenResource`] pointing at the concrete file
//!   under the MSL cache.
//!
//! Registered from [`ModelicaCommandsPlugin::build`]
//! (`ui/commands.rs`); the observer in this module then translates
//! `UriClicked` with `doc_kind == "modelica"` into the existing
//! [`OpenClass`] command — so clicks in Documentation reuse the same
//! drill-in pipeline as clicks from the Welcome tab or the canvas.

use std::path::PathBuf;

use bevy::prelude::*;
use lunco_workbench::{UriClicked, UriHandler, UriResolution};

/// Handler for `modelica://` URIs.
pub struct ModelicaUriHandler;

impl UriHandler for ModelicaUriHandler {
    fn scheme(&self) -> &'static str {
        "modelica"
    }

    fn resolve(&self, uri: &str) -> UriResolution {
        // Strip `modelica://` prefix case-insensitively. `UriRegistry`
        // lower-cases the scheme for dispatch, but the rest of the
        // URI is passed through verbatim — tolerant of `Modelica://`
        // we sometimes see in older MSL docs.
        let Some(body) = uri
            .strip_prefix("modelica://")
            .or_else(|| uri.strip_prefix("Modelica://"))
        else {
            return UriResolution::NotHandled;
        };

        // Split off anchor fragment (`#Overview` → "Overview") before
        // parsing so the anchor doesn't end up attached to a class
        // path or filename.
        let (body, anchor) = match body.split_once('#') {
            Some((b, a)) => (b, Some(a.to_string())),
            None => (body, None),
        };

        if body.is_empty() {
            return UriResolution::NotHandled;
        }

        // Split on the FIRST `/`. Modelica convention:
        //   `A.B.C`              → class reference (no slash)
        //   `A.B.C/path/to/file` → resource under class A.B.C's
        //                          package directory (slash present)
        match body.split_once('/') {
            Some((class_dotted, subpath)) => resolve_resource(class_dotted, subpath),
            None => {
                if let Some(anchor) = anchor {
                    UriResolution::NavigateAnchor {
                        identifier: body.to_string(),
                        anchor,
                    }
                } else {
                    UriResolution::OpenDocument {
                        doc_kind: "modelica",
                        identifier: body.to_string(),
                    }
                }
            }
        }
    }
}

/// Compute the on-disk path for a `modelica://Pkg.Sub/Resources/...`
/// URI. Walks down from the MSL cache root (`lunco_assets::msl_dir()`)
/// using each dotted segment as a directory. Packages outside the
/// MSL tree (user workspace libraries) aren't resolved yet — that's
/// a follow-up once the Twin / user-library path map is available
/// from the workbench crate.
fn resolve_resource(class_dotted: &str, subpath: &str) -> UriResolution {
    let msl_root = lunco_assets::msl_dir();
    let mut path: PathBuf = msl_root;
    for segment in class_dotted.split('.') {
        if segment.is_empty() {
            return UriResolution::NotHandled;
        }
        path.push(segment);
    }
    for segment in subpath.split('/') {
        if segment.is_empty() {
            continue;
        }
        path.push(segment);
    }
    if path.exists() {
        UriResolution::OpenResource { path }
    } else {
        // Degrade to `External` rather than `NotHandled` — the
        // renderer can then show the raw URI + a tooltip telling the
        // user the resource wasn't found, rather than silently
        // swallowing the click.
        UriResolution::NotHandled
    }
}

/// Observer: translate `UriClicked` events for `modelica://` URIs
/// into the concrete [`OpenClass`](crate::ui::commands::OpenClass)
/// command. Non-Modelica resolutions are ignored — other domain
/// observers pick them up. Resource opens + anchor navigation are
/// logged at `debug` for now (no domain command yet wires them; a
/// follow-up task picks those up).
pub fn on_modelica_uri_clicked(
    trigger: On<UriClicked>,
    mut commands: Commands,
) {
    let ev = trigger.event();
    match &ev.resolution {
        UriResolution::OpenDocument {
            doc_kind: "modelica",
            identifier,
        } => {
            commands.trigger(crate::ui::commands::OpenClass {
                qualified: identifier.clone(),
            });
        }
        UriResolution::NavigateAnchor { identifier, anchor } => {
            // TODO: anchor navigation inside the active Docs view.
            // Until then, open the class and drop the anchor — keeps
            // the link functional even if it doesn't scroll to the
            // specific section.
            bevy::log::debug!(
                "modelica URI anchor not yet wired: {identifier}#{anchor}"
            );
            commands.trigger(crate::ui::commands::OpenClass {
                qualified: identifier.clone(),
            });
        }
        UriResolution::OpenResource { path } => {
            // TODO: pipe to a generic "open this file" command once
            // the workbench has one. For MSL Documentation links
            // most resources are images already rendered inline, so
            // this path mostly fires for intentional right-click
            // "open resource" flows we haven't built yet.
            bevy::log::debug!(
                "modelica URI resource click (not yet routed): {}",
                path.display()
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_ref_resolves_to_open_document() {
        let h = ModelicaUriHandler;
        assert_eq!(
            h.resolve("modelica://Modelica.Blocks.Examples.PID_Controller"),
            UriResolution::OpenDocument {
                doc_kind: "modelica",
                identifier: "Modelica.Blocks.Examples.PID_Controller".to_string(),
            }
        );
    }

    #[test]
    fn anchor_resolves_to_navigate_anchor() {
        let h = ModelicaUriHandler;
        assert_eq!(
            h.resolve("modelica://Modelica.Blocks#Overview"),
            UriResolution::NavigateAnchor {
                identifier: "Modelica.Blocks".to_string(),
                anchor: "Overview".to_string(),
            }
        );
    }

    #[test]
    fn missing_prefix_returns_not_handled() {
        let h = ModelicaUriHandler;
        assert_eq!(h.resolve("https://example.com"), UriResolution::NotHandled);
    }

    #[test]
    fn empty_body_returns_not_handled() {
        let h = ModelicaUriHandler;
        assert_eq!(h.resolve("modelica://"), UriResolution::NotHandled);
    }

    #[test]
    fn case_insensitive_scheme() {
        let h = ModelicaUriHandler;
        assert!(matches!(
            h.resolve("Modelica://Foo"),
            UriResolution::OpenDocument { .. }
        ));
    }
}
