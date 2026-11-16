//! Platform action admission for the primary workbench accessibility tree.

use bevy::a11y::{ActionRequest, ManageAccessibilityUpdates};
use bevy::prelude::*;
use bevy_egui::{EguiInput, EguiPreUpdateSet, PrimaryEguiContext, egui};

/// The maintained egui adapter publishes its tree. The host delivers platform
/// action requests through egui's public RawInput event API while that tree owns
/// the platform adapter. Native Bevy nodes consume only their own tree's events.
fn forward_platform_actions(
    mut requests: MessageReader<ActionRequest>,
    native_tree: Res<ManageAccessibilityUpdates>,
    mut primary: Query<&mut EguiInput, With<PrimaryEguiContext>>,
) {
    if native_tree.get() {
        requests.clear();
        return;
    }
    let Ok(mut input) = primary.single_mut() else {
        return;
    };
    for request in requests.read() {
        input
            .events
            .push(egui::Event::AccessKitActionRequest(request.0.clone()));
    }
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<ManageAccessibilityUpdates>()
        .add_message::<ActionRequest>()
        .add_systems(
            PreUpdate,
            forward_platform_actions
                .after(EguiPreUpdateSet::ProcessInput)
                .before(EguiPreUpdateSet::BeginPass),
        );
}
