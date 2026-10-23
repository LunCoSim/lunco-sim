//! Render acknowledgement for loading overlays on a captured UI surface.
//!
//! A cleared domain busy flag does not prove that a previously painted loading
//! card has left the rendered image. The UI publishes what it painted; capture
//! waits for that clean revision to complete the render schedule.

use bevy::prelude::*;
use bevy::render::{ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems};

/// Present only in hosts that render a UI into the capture surface.
/// The UI owns visibility; the render world owns the clean acknowledgement.
#[derive(Resource, Debug)]
pub struct CaptureUiPresentation {
    revision: u64,
    loading_visible: bool,
    submitted_clean_revision: Option<u64>,
}

impl Default for CaptureUiPresentation {
    fn default() -> Self {
        Self {
            revision: 0,
            loading_visible: true,
            submitted_clean_revision: None,
        }
    }
}

impl CaptureUiPresentation {
    /// Publish whether the current UI pass paints a loading indicator.
    pub fn set_loading_visible(&mut self, visible: bool) {
        if self.loading_visible != visible {
            self.loading_visible = visible;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// A completed clean render must match the current visibility revision.
    pub fn ready(&self) -> bool {
        !self.loading_visible && self.submitted_clean_revision == Some(self.revision)
    }
}

#[derive(Resource, Default)]
struct UiRenderAck {
    extracted: Option<u64>,
    submitted: Option<u64>,
}

fn extract_ui_presentation(mut main: ResMut<MainWorld>, mut ack: ResMut<UiRenderAck>) {
    if let Some(mut presentation) = main.get_resource_mut::<CaptureUiPresentation>() {
        presentation.submitted_clean_revision = ack.submitted;
        ack.extracted = (!presentation.loading_visible).then_some(presentation.revision);
    } else {
        ack.extracted = None;
    }
}

fn acknowledge_ui_submission(mut ack: ResMut<UiRenderAck>) {
    ack.submitted = ack.extracted;
}

/// Install the render half; hosts without a captured UI need no acknowledgement.
pub(crate) fn install(app: &mut App) {
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<UiRenderAck>()
        .add_systems(ExtractSchedule, extract_ui_presentation)
        .add_systems(
            Render,
            acknowledge_ui_submission.after(RenderSystems::Render),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_and_stale_clean_submissions_never_admit_capture() {
        let mut state = CaptureUiPresentation::default();
        assert!(!state.ready());
        state.set_loading_visible(false);
        assert!(!state.ready());
        state.submitted_clean_revision = Some(state.revision);
        assert!(state.ready());
        state.set_loading_visible(true);
        assert!(!state.ready());
        state.set_loading_visible(false);
        assert!(
            !state.ready(),
            "the earlier clean frame cannot acknowledge a new overlay cycle"
        );
        state.submitted_clean_revision = Some(state.revision);
        assert!(state.ready());
    }
}
