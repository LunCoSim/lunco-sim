//! Stamp the source revision into the shared LunCoSim runtime.

mod build_identity {
    include!("../../scripts/build_identity.rs");
}

fn main() {
    build_identity::stamp();
}
