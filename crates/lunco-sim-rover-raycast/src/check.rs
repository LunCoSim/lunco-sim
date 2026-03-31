use avian3d::prelude::*;

fn check() {
    let _ = ConstantLocalTorque(avian3d::math::Vector::ZERO);
    // let _ = ExternalForce::default(); // This failed last time
}
