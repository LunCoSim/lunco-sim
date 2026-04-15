//! Telemetry subscription system — streams telemetry events to API subscribers.

use bevy::prelude::*;
use crate::{
    executor::ApiResponseEvent,
    schema::{ApiResponse, TelemetryFilter, TelemetryResponse},
};

/// Active telemetry subscription.
#[derive(Debug)]
pub struct TelemetrySubscription {
    pub id: u64,
    pub filter: TelemetryFilter,
}

/// Registry of active telemetry subscriptions.
#[derive(Resource, Default)]
pub struct TelemetrySubscriptions {
    subscriptions: Vec<TelemetrySubscription>,
    next_id: u64,
    next_correlation_id: u64,
}

impl TelemetrySubscriptions {
    pub fn subscribe(&mut self, filter: Option<TelemetryFilter>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.subscriptions.push(TelemetrySubscription { id, filter: filter.unwrap_or_default() });
        id
    }
    pub fn unsubscribe(&mut self, id: u64) {
        self.subscriptions.retain(|s| s.id != id);
    }
    fn should_broadcast(&self, name: &str, severity: Option<lunco_core::Severity>) -> bool {
        if self.subscriptions.is_empty() { return false; }
        self.subscriptions.iter().any(|sub| {
            let name_ok = sub.filter.names.is_empty() || sub.filter.names.contains(&name.to_string());
            let severity_ok = match (severity, &sub.filter.min_severity) {
                (None, _) => true,
                (Some(_), None) => true,
                (Some(sev), Some(min_str)) => {
                    let min = match min_str.as_str() {
                        "Debug" => lunco_core::Severity::Debug,
                        "Info" => lunco_core::Severity::Info,
                        "Warning" => lunco_core::Severity::Warning,
                        "Error" => lunco_core::Severity::Error,
                        "Critical" => lunco_core::Severity::Critical,
                        _ => lunco_core::Severity::Debug,
                    };
                    sev >= min
                }
            };
            name_ok && severity_ok
        })
    }
    fn next_correlation_id(&mut self) -> u64 {
        let id = self.next_correlation_id;
        self.next_correlation_id += 1;
        id
    }
}

/// Observer for sampled parameters.
pub fn sampled_param_observer(
    trigger: On<lunco_core::telemetry::SampledParameter>,
    mut subscriptions: ResMut<TelemetrySubscriptions>,
    mut commands: Commands,
) {
    let sample = trigger.event();
    if !subscriptions.should_broadcast(&sample.name, None) { return; }
    let correlation_id = subscriptions.next_correlation_id();
    commands.trigger(ApiResponseEvent {
        correlation_id,
        response: ApiResponse::TelemetryEvent(TelemetryResponse::from_sampled(sample)),
    });
}

/// Observer for telemetry events.
pub fn telemetry_event_observer(
    trigger: On<lunco_core::telemetry::TelemetryEvent>,
    mut subscriptions: ResMut<TelemetrySubscriptions>,
    mut commands: Commands,
) {
    let event = trigger.event();
    if !subscriptions.should_broadcast(&event.name, Some(event.severity)) { return; }
    let correlation_id = subscriptions.next_correlation_id();
    commands.trigger(ApiResponseEvent {
        correlation_id,
        response: ApiResponse::TelemetryEvent(TelemetryResponse::from_event(event)),
    });
}

/// Plugin that registers telemetry subscription observers.
pub struct ApiTelemetryPlugin;
impl Plugin for ApiTelemetryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TelemetrySubscriptions>()
            .add_observer(sampled_param_observer)
            .add_observer(telemetry_event_observer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscribe_unsubscribe() {
        let mut subs = TelemetrySubscriptions::default();
        let id = subs.subscribe(None);
        subs.unsubscribe(id);
        assert!(subs.subscriptions.is_empty());
    }

    #[test]
    fn test_broadcast_with_default_filter() {
        let mut subs = TelemetrySubscriptions::default();
        subs.subscribe(None);
        assert!(subs.should_broadcast("any_name", None));
        assert!(subs.should_broadcast("any_name", Some(lunco_core::Severity::Critical)));
    }

    #[test]
    fn test_severity_filter() {
        let mut subs = TelemetrySubscriptions::default();
        subs.subscribe(Some(TelemetryFilter {
            names: vec![],
            min_severity: Some("Warning".to_string()),
        }));
        assert!(!subs.should_broadcast("alert", Some(lunco_core::Severity::Debug)));
        assert!(subs.should_broadcast("alert", Some(lunco_core::Severity::Warning)));
        assert!(subs.should_broadcast("alert", Some(lunco_core::Severity::Critical)));
    }

    #[test]
    fn test_name_filter() {
        let mut subs = TelemetrySubscriptions::default();
        subs.subscribe(Some(TelemetryFilter {
            names: vec!["motor_temp".to_string()],
            min_severity: None,
        }));
        assert!(subs.should_broadcast("motor_temp", None));
        assert!(!subs.should_broadcast("other_param", None));
    }
}
