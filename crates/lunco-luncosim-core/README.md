# lunco-luncosim-core

Headless-safe LunCoSim simulation substrate. This is the generic production
core shared by the windowed `luncosim` shell, `luncosim-server`, and the
authored scene-test runner.

It owns simulation composition: the persistent world shell, Avian physics, USD
loading and projection, Modelica/cosimulation, networking/API integration,
runtime exposures, persistence, and generic headless scheduling primitives. It
does not own Rhai policy projection, scripting journal consumers, renderer/window
configuration, egui, picking, workbench presentation, or tutorial policy.

The package intentionally has no UI feature. The server therefore depends on
this package directly, while the GUI shell adds `lunco-luncosim-ui` and the
rendering stack at its application boundary. Feature changes in the GUI do not
invalidate the server's application crate.

```bash
cargo check -p lunco-luncosim-core -j 4
cargo build -p lunco-luncosim-server --bin luncosim-server -j 4
```

`build_core_app_with_scene` is the low-level headless substrate constructor.
`lunco-luncosim-runtime` adds the production Rhai/policy boundary and owns the
public headless launcher and scene-test composition. Authored behavior and
asset-backed assertions belong in Rhai scene tests; Rust tests in this package
cover only low-level mechanisms that the authored surface cannot observe
directly.
