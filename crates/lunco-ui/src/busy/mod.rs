//! Busy / loading indicator widgets backed by [`lunco_status_core::status_bus`].
//!
//! Panels never render spinners directly. They ask
//! [`LoadingIndicator::for_scope`] whether the requested scope is busy,
//! and the widget chooses whether and how to paint based on shared
//! best-practice constants (delay before showing, threshold before
//! displaying elapsed time, etc.).
//!
//! In-flight work is registered via [`StatusBus::begin`], which returns
//! a `BusyHandle` whose `Drop` clears the entry on the next frame. Panels
//! never carry `is_loading: bool` flags.

pub mod spinner;
pub mod widget;

pub use widget::LoadingIndicator;

pub use lunco_status_core::status_bus::{
    BusyHandle, BusyId, BusyScope, StatusBus, StatusEvent, StatusLevel,
};
