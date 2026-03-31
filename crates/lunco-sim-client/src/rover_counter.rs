use bevy::prelude::*;
use lunco_sim_core::Vessel;

pub struct RoverCountPlugin;

impl Plugin for RoverCountPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, count_rovers);
    }
}

fn count_rovers(q_rovers: Query<&Name, With<Vessel>>) {
    let mut names = Vec::new();
    for name in q_rovers.iter() {
        names.push(name.as_str());
    }
    if !names.is_empty() {
        println!("Currently active rovers: {:?}", names);
    }
}
