# lunco-luncosim-core

Dependency-light Bevy substrate shared by the windowed `luncosim` shell,
`luncosim-server`, and the authored scene-test runner.

It owns only the host-neutral headless plugin group, asset source/type
registration, task-pool policy, build identity, and log deduplication. Physics,
USD, terrain, Modelica/cosimulation, celestial, avatar, and scene-command
composition live in `lunco-luncosim-simulation`.

The package intentionally has no UI feature. The server therefore depends on
this package directly, while the GUI shell adds `lunco-luncosim-ui` and the
rendering stack at its application boundary. Feature changes in the GUI do not
invalidate the server's application crate.

```bash
cargo check -p lunco-luncosim-core -j 4
cargo build -p lunco-luncosim-server --bin luncosim-server -j 4
```

`build_core_app` is the low-level substrate constructor.
`lunco-luncosim-runtime` installs the simulation composition, application
services, and production Rhai/policy boundary, then owns the public headless
launcher and scene-test composition. Authored behavior and asset-backed
assertions belong in Rhai scene tests; Rust tests in this package cover only
the substrate mechanisms that the authored surface cannot observe directly.
