use bevy::prelude::*;
use crate::architecture::*;

/// To satisfy the Testability Mandate (FR-010) we use Mocks
/// These structs are used during unit testing and integration testing 
/// without needing the full f64 physics environment or rendering.

pub struct MockObcPlugin;
impl Plugin for MockObcPlugin {
    fn build(&self, app: &mut App) {
        // Mock OBC might inject diagnostic systems directly monitoring DigitalPorts, OR simply setup test rigs.
    }
}

pub struct MockPlantPlugin;
impl Plugin for MockPlantPlugin {
    fn build(&self, app: &mut App) {
        // Tracks physical outputs from the Tier 2 scaling loop correctly
    }
}

/// Allows tracking Physical outputs (Tiers 2 assertion)
#[derive(Component)]
pub struct ValueTracker {
    pub history: Vec<f32>,
}
