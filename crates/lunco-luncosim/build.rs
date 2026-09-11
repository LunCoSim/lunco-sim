//! Stamp the build's git revision into the application binary.
//!
//! A tester's log has to identify the build that produced it. Five Windows tester
//! runs in the 2026-07-26 report could only be told apart by install path and asset
//! counts, which is why that report has a "build grouping" paragraph instead of a
//! build column.
//!
//! Failure is not fatal: a source tarball or a vendored build has no git, and a
//! simulator that refuses to compile outside a checkout would be worse than one whose
//! log says `unknown`.

mod build_identity {
    include!("../../scripts/build_identity.rs");
}

fn main() {
    build_identity::stamp();
}
