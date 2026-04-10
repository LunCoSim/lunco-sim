use bevy::prelude::*;
use big_space::prelude::*;

#[test]
fn test_query() {
    let mut app = App::new();
    app.add_systems(Update, my_system);
}

fn my_system(
    q: Query<(Entity, &CellCoord, &Transform, &ChildOf)>,
    q_grids: Query<&Grid>,
) {
    for (entity, _cell, _tf, child_of) in q.iter() {
        let parent = child_of.parent();
        let _ = q_grids.get(parent);
    }
}
