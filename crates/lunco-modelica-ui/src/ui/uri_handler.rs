//! Modelica link interpretation. Resource links retain their URI and enter the
//! shared workbench preview; class links use the existing class-open command.

use bevy::prelude::*;
use lunco_modelica_ast::resource_uri::ModelicaUri;
use lunco_workbench_core::uri::{UriClicked, UriHandler, UriResolution};

/// Modelica link semantics; resource I/O runs through the shared asset loader.
pub struct ModelicaUriHandler;
impl UriHandler for ModelicaUriHandler {
    fn scheme(&self) -> &'static str {
        "modelica"
    }
    fn resolve(&self, uri: &str) -> UriResolution {
        match ModelicaUri::parse(uri) {
            Ok(Some(parsed)) if parsed.resource.is_some() => UriResolution::OpenResource {
                uri: uri.to_owned(),
            },
            Ok(Some(parsed)) => match parsed.fragment {
                Some(anchor) => UriResolution::NavigateAnchor {
                    identifier: parsed.qualified,
                    anchor,
                },
                None => UriResolution::OpenDocument {
                    doc_kind: "modelica",
                    identifier: parsed.qualified,
                },
            },
            Ok(None) => UriResolution::NotHandled,
            Err(message) => UriResolution::Rejected { message },
        }
    }
}

/// Dispatch class links through the existing Modelica class owner.
pub fn on_modelica_uri_clicked(trigger: On<UriClicked>, mut commands: Commands) {
    let ev = trigger.event();
    match &ev.resolution {
        UriResolution::OpenDocument {
            doc_kind: "modelica",
            identifier,
        } => {
            commands.trigger(lunco_modelica_ui_core::OpenClass {
                qualified: identifier.clone(),
                action: lunco_modelica_ui_core::ClassAction::View,
            });
        }
        UriResolution::NavigateAnchor { identifier, anchor } => {
            // TODO: anchor navigation inside the active Docs view.
            // Until then, open the class and drop the anchor — keeps
            // the link functional even if it doesn't scroll to the
            // specific section.
            bevy::log::debug!("modelica URI anchor not yet wired: {identifier}#{anchor}");
            commands.trigger(lunco_modelica_ui_core::OpenClass {
                qualified: identifier.clone(),
                action: lunco_modelica_ui_core::ClassAction::View,
            });
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
    fn empty_body_is_rejected() {
        let h = ModelicaUriHandler;
        assert!(matches!(
            h.resolve("modelica://"),
            UriResolution::Rejected { .. }
        ));
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
