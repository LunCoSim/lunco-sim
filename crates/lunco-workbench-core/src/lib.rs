//! Renderer-independent contracts for the LunCoSim workbench.
//!
//! This crate contains the panel, menu, perspective, and read-model contracts
//! shared by domain UI crates and the concrete workbench shell. It deliberately
//! does not depend on `bevy_egui`, `egui_dock`, a renderer, a window, storage,
//! or application services.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod menu;
mod panel;
mod perspective;
mod snapshot;

pub use menu::{
    CustomMenu, MenuCallback, MenuCtx, MenuIntents, SettingsSubmenu, UndoProbe, UndoProbeCtx,
    WorkbenchMenuRegistry,
};
pub use panel::{
    InstancePanel, InstancePanelMenuEntry, Panel, PanelCtx, PanelId, PanelIntents, PanelMenuGroup,
    PanelRenderTarget, PanelScrollPolicy, PanelSlot, PanelSurfaceStyle, TabId,
};
pub use perspective::{
    Perspective, PerspectiveId, PerspectiveInstanceTab, PerspectiveLayoutPlan, PerspectiveSlotPlan,
};
pub use snapshot::WorkbenchSnapshot;
