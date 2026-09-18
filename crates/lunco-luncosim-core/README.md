# lunco-luncosim-core

Headless-safe LunCoSim simulation substrate. This is the generic production
core shared by the windowed `luncosim` shell, `luncosim-server`, and the
authored scene-test runner.

It owns simulation composition: the persistent world shell, Avian physics, USD
loading and projection, Modelica/cosimulation, runtime exposures, and generic
headless scheduling primitives. Application services such as startup Twin
resolution, API transport, networking, journal projection, and persistence live
in `lunco-luncosim-services`.

The package intentionally has no UI feature. The server therefore depends on
this package directly, while the GUI shell adds `lunco-luncosim-ui` and the
rendering stack at its application boundary. Feature changes in the GUI do not
invalidate the server's application crate.

```bash
cargo check -p lunco-luncosim-core -j 4
cargo build -p lunco-luncosim-server --bin luncosim-server -j 4
```

`build_core_app` is the low-level headless substrate constructor.
`lunco-luncosim-runtime` composes the application services and production
Rhai/policy boundary, then owns the public headless launcher and scene-test
composition. Authored behavior and asset-backed assertions belong in Rhai scene
tests; Rust tests in this package cover only low-level mechanisms that the
authored surface cannot observe directly.
