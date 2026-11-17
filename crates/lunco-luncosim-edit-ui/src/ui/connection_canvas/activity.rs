//! Bounded read-only projection of existing binding and port owners.
use super::{UsdCanvasState, workspace::BindingRow};
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_cosim_core::{ConnectionBinding, CosimDiagnostics, PortHolds, SimConnection};
use lunco_port_core::ports::PortRegistry;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct Activity {
    sampled_at: Option<f64>,
    fixed_time: Option<f64>,
    pub running: bool,
    pub requested: HashSet<String>,
    pub ports: HashMap<String, Sample>,
    bindings: HashMap<(String, String), Binding>,
}

struct Binding {
    state: ConnectionBinding,
    held: bool,
    failure: Option<String>,
}
pub(super) struct Sample {
    pub value: Option<f64>,
    pub unit: Option<String>,
    pub changed_at: Option<f64>,
}
impl Sample {
    pub(super) fn display(&self) -> String {
        let value = super::super::ports::format_value(self.value);
        self.unit
            .as_ref()
            .map(|unit| format!("{value} {unit}"))
            .unwrap_or(value)
    }
}
impl Activity {
    pub(super) fn clear_samples(&mut self) {
        self.ports.clear();
        self.requested.clear();
    }
    pub(super) fn status(&self, document: bool, row: &BindingRow) -> &str {
        if document {
            return "document";
        }
        if row.kind != super::projection::WireKind::Dataflow {
            return "structural";
        }
        match self.bindings.get(&(row.source.clone(), row.target.clone())) {
            Some(binding) if binding.failure.is_some() => "failed",
            Some(binding) => match binding.state {
                ConnectionBinding::Pending => "pending",
                ConnectionBinding::Failed => "failed",
                ConnectionBinding::Bound if binding.held => "held",
                ConnectionBinding::Bound => "bound",
            },
            None => "not_projected",
        }
    }
    pub(super) fn failure(&self, row: &BindingRow) -> Option<&str> {
        self.bindings
            .get(&(row.source.clone(), row.target.clone()))
            .and_then(|b| b.failure.as_deref())
    }
    pub(super) fn animating(&self, row: &BindingRow, now: f64, enabled: bool) -> bool {
        enabled
            && self.running
            && self.status(false, row) == "bound"
            && self
                .ports
                .get(&row.source)
                .and_then(|p| p.changed_at)
                .is_some_and(|at| now >= at && now - at < 0.6)
    }
}

pub fn sample_due(
    time: Res<Time<Real>>,
    views: Res<UsdCanvasState>,
    layout: Option<Res<lunco_workbench_core::WorkbenchSnapshot>>,
) -> bool {
    layout.is_some_and(|layout| layout.is_panel_visible(super::USD_CANVAS_PANEL_ID))
        && views.show_scene
        && views.scene.built
        && views
            .scene
            .activity
            .sampled_at
            .is_none_or(|at| time.elapsed_secs_f64() - at >= 0.1)
}

/// Registry reads occur outside paint, only for endpoints requested by visible
/// rows or selected wires. A document preview never borrows a live scene's data.
pub fn sample(world: &mut World) {
    let Some(registry) = world.get_resource::<PortRegistry>().cloned() else {
        return;
    };
    let now = world.resource::<Time<Real>>().elapsed_secs_f64();
    let fixed = world.resource::<Time<Fixed>>().elapsed_secs_f64();
    let clock_running = world
        .get_resource::<Time<Virtual>>()
        .is_some_and(|t| !t.is_paused() && t.relative_speed_f64() > 0.0);
    let specs: Vec<_> = world
        .query::<(&SimConnection, &ConnectionBinding)>()
        .iter(world)
        .map(|(spec, binding)| (spec.clone(), binding.clone()))
        .collect();
    world.resource_scope(|world, mut views: Mut<UsdCanvasState>| {
        let state = &mut views.scene;
        let progressing = clock_running
            && state
                .activity
                .fixed_time
                .is_some_and(|previous| fixed > previous);
        let mut bindings = HashMap::new();
        for (spec, binding) in specs {
            let Some(source) = world.get::<lunco_usd_bevy_scene::UsdPrimPath>(spec.start_element)
            else {
                continue;
            };
            let Some(target) = world.get::<lunco_usd_bevy_scene::UsdPrimPath>(spec.end_element)
            else {
                continue;
            };
            if state.entities.get(&source.path) != Some(&spec.start_element)
                || state.entities.get(&target.path) != Some(&spec.end_element)
            {
                continue;
            }
            let held = world
                .get_resource::<PortHolds>()
                .is_some_and(|h| h.get(spec.end_element, &spec.end_connector).is_some());
            let failure = world
                .get_resource::<CosimDiagnostics>()
                .and_then(|d| d.faults.get(&spec.end_element))
                .and_then(|ports| ports.get(spec.end_connector.as_str()))
                .and_then(|fault| fault.failure.clone());
            bindings.insert(
                (
                    format!(
                        "{}.{}:{}",
                        source.path,
                        if spec.start_is_input {
                            "inputs"
                        } else {
                            "outputs"
                        },
                        spec.start_connector
                    ),
                    format!("{}.inputs:{}", target.path, spec.end_connector),
                ),
                Binding {
                    state: binding,
                    held,
                    failure,
                },
            );
        }
        state.activity.bindings = bindings;
        let requested = std::mem::take(&mut state.activity.requested);
        state
            .activity
            .ports
            .retain(|key, _| requested.contains(key));
        for key in requested {
            let Some((path, property)) = key.rsplit_once('.') else {
                continue;
            };
            let Some((namespace, name)) = property.split_once(':') else {
                continue;
            };
            let Some(entity) = state.entities.get(path).copied() else {
                continue;
            };
            let (value, metadata) = match namespace {
                "inputs" => (
                    registry.read_input_port(world, entity, name),
                    registry.input_port_metadata(world, entity, name),
                ),
                "outputs" => (
                    registry.read_output_port(world, entity, name),
                    registry.output_port_metadata(world, entity, name),
                ),
                _ => continue,
            };
            let unit = metadata
                .and_then(|m| m.unit)
                .map(|unit| unit.id().to_string());
            let previous = state.activity.ports.get(&key);
            let changed = progressing
                && previous
                    .and_then(|p| p.value)
                    .zip(value)
                    .is_some_and(|(a, b)| {
                        a.is_finite() && b.is_finite() && a.to_bits() != b.to_bits()
                    });
            let changed_at = if changed {
                Some(now)
            } else {
                previous.and_then(|p| p.changed_at)
            };
            state.activity.ports.insert(
                key,
                Sample {
                    value,
                    unit,
                    changed_at,
                },
            );
        }
        state.activity.sampled_at = Some(now);
        state.activity.fixed_time = Some(fixed);
        state.activity.running = progressing;
    });
}

/// An observed-change pulse uses elapsed time since an actual sampled change.
/// Its distance is decorative, never a physical velocity or transfer rate.
pub(super) fn paint(
    ui: &mut egui::Ui,
    state: &mut super::UsdCanvasSessionState,
    rect: egui::Rect,
    now: f64,
) {
    if !state.workspace.live_values {
        return;
    }
    let trace = super::workspace::SelectionTrace::new(state);
    let selected: Vec<_> = state
        .workspace
        .rows
        .iter()
        .filter(|row| trace.matches(row))
        .cloned()
        .collect();
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let theme = lunco_theme::active(ui.ctx());
    for row in selected {
        state.activity.requested.insert(row.source.clone());
        state.activity.requested.insert(row.target.clone());
        let Some(edge) = state.canvas.scene.edge(row.id) else {
            continue;
        };
        let Some((from, to)) = state.canvas.scene.edge_endpoint_positions(edge) else {
            continue;
        };
        let screen = lunco_canvas::Rect::from_min_max(
            lunco_canvas::Pos::new(rect.min.x, rect.min.y),
            lunco_canvas::Pos::new(rect.max.x, rect.max.y),
        );
        let points: Vec<_> = std::iter::once(from)
            .chain(edge.waypoints.iter().copied())
            .chain(std::iter::once(to))
            .map(|point| state.canvas.viewport.world_to_screen(point, screen))
            .collect();
        let Some(segment) = points.windows(2).max_by(|a, b| {
            let length = |p: &[lunco_canvas::Pos]| (p[1].x - p[0].x).hypot(p[1].y - p[0].y);
            length(a).total_cmp(&length(b))
        }) else {
            continue;
        };
        let center = egui::pos2(
            (segment[0].x + segment[1].x) * 0.5,
            (segment[0].y + segment[1].y) * 0.5,
        );
        if let Some(value) = state.activity.ports.get(&row.source) {
            painter.text(
                center,
                egui::Align2::CENTER_BOTTOM,
                value.display(),
                lunco_theme::TypographyRole::DenseData.font_id(ui.style()),
                theme.tokens.text,
            );
        }
        if state
            .activity
            .animating(&row, now, state.workspace.animate_changes)
        {
            let age = state
                .activity
                .ports
                .get(&row.source)
                .and_then(|sample| sample.changed_at)
                .map(|at| ((now - at) / 0.6).clamp(0.0, 1.0) as f32)
                .unwrap_or_default();
            painter.circle_stroke(
                center,
                theme.spacing.item_spacing * (1.0 + age),
                egui::Stroke::new(
                    2.0,
                    theme.tokens.node_border_selected.gamma_multiply(1.0 - age),
                ),
            );
        }
    }
}
