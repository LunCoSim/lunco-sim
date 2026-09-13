# lunco-luncosim-core

Headless-safe LunCoSim application runtime. This is the production substrate
shared by the windowed `luncosim` shell, `luncosim-server`, and the authored
scene-test runner.

It owns simulation composition: the persistent world shell, Avian physics, USD
loading and projection, Modelica/cosimulation, networking/API integration,
runtime exposures, persistence, and the headless schedule runner. It does not
own renderer/window configuration, egui, picking, workbench presentation, or
tutorial policy.

The package intentionally has no UI feature. The server therefore depends on
this package directly, while the GUI shell adds `lunco-luncosim-ui` and the
rendering stack at its application boundary. Feature changes in the GUI do not
invalidate the server's application crate.

```bash
cargo check -p lunco-luncosim-core -j 4
cargo build -p lunco-luncosim-server --bin luncosim-server -j 4
```

`build_headless_app_with_threads` is the composition entry point for bounded
scene tests and `run_headless` is the production launcher entry point. Authored
behavior and asset-backed assertions belong in Rhai scene tests; Rust tests in
this package cover only low-level mechanisms that the authored surface cannot
observe directly.
