# LunCoSim

> **Try it live:** [**sandbox.lunco.space**](https://sandbox.lunco.space) — runs in the browser. Early preview build; expect rough edges.

**LunCoSim** is the LunCo ground-mobility application for testing physics interactions and scene composition. It is the primary tool for validating rover chassis, suspension behavior, and environment collision.

## What it does

- **Physics Validation**: Test Avian3D physics in a controlled environment.
- **Mobility Testing**: Drive rovers with different wheel types (raycast vs. physical).
- **Scene Preview**: Load and inspect USD scenes synchronously.
- **Modeling & Cosim**: The full Modelica IDE — the same workbench the standalone
  *lunica* app provides — is embedded as the **Design workspace**. Open, edit,
  compile, and run `.mo` models with live plots. Models can be co-simulated with
  the physics: a Modelica model holds the control law, a `SimConnection` pipes its
  outputs to a physics body, and state flows back in. The bundled lander flies
  this way; see the [Cosim walkthrough](../../tutorials/03-cosim.md).
- **Networked Play**: Can act as a listen-server or client for multiplayer testing.

## Runtime-authored UI

Twin-facing HUDs and lightweight overlays can be authored under `assets/ui/`
with the retained HTML/CSS-like surface layer. Start with the
[runtime-authored UI architecture guide](../../architecture/runtime-authored-ui.md)
and the [runtime UI skill](../../../skills/runtime-ui/SKILL.md). Edit the HTML
or CSS while a native `luncosim` session is running; the asset watcher reloads
those files without rebuilding or relaunching the simulator. Engine values must
be published through the generic `EngineExposures` capability registry, and
buttons map to semantic actions handled by the existing typed command path.

This is a small native UI language, not browser HTML: use the workbench/egui
panels for rich editors, text input, large inspectors, and controls whose
semantics are not in the runtime surface contract.

## Workspaces

The luncosim has two workspaces, switched via the tabs at the top of the window:

- **View** (default) — the 3D scene: viewport, spawn palette, inspector, rover
  driving, telemetry. Covered by the *Sandbox Intro → First Drive → Lander & Rover
  Mission* tutorials and the *Build a Scene → Script a Rover → Inspect the
  Simulation → Cosim* authoring track.
- **Design** — the embedded Modelica IDE: source / diagram / icon / docs views,
  a component palette, compile & run (interactive and fast), live inputs, and
  plots. Backed by the [Modelica Standard Library](https://github.com/modelica/ModelicaStandardLibrary)
  plus the bundled models in `assets/models/` (`Lander.mo`, `Battery.mo`,
  `QuarterCar.mo`, …). Open `Lander.mo` here to see the law the lander flies.

### Desktop updates

The official installers are platform-specific:

| Platform | Download | Launch after installation |
|---|---|---|
| Windows x86_64 | `LunCoSim-Windows-x86_64-Setup.exe` | The installed Start-menu or desktop shortcut |
| macOS Apple Silicon | `LunCoSim-macOS-Apple-Silicon.pkg` | The installed `LunCoSim.app` |
| macOS Intel | `LunCoSim-macOS-Intel.pkg` | The installed `LunCoSim.app` |
| Linux x86_64 | `LunCoSim-Linux-x86_64.AppImage` | The same AppImage from a writable location |

LunCoSim checks the matching runtime feed once when the GUI starts. When a new
version is found, the status bar shows a red, clickable update notice. Click
**Download update** to start or retry the download; the status bar reports the
percentage while you keep working. When the package is ready, click
**Install and restart**. The Settings → Updates menu remains available for
manual checks and update preferences.

Update checks and downloads use bounded network requests. A slow or unavailable
connection leaves the current installation untouched, reports a recoverable
error instead of claiming that no update exists, and exposes **Check again** or
**Retry download**. The first download phase is shown as an indeterminate
connection state rather than a misleading frozen 0%.

Windows updates replace the installed application managed by Velopack. macOS
updates replace the installed `.app` bundle. Linux updates replace the same
writable AppImage. Always launch the installed package or same AppImage rather
than `Setup.exe`, a copied `.app` binary, `target/debug/luncosim`, a source
build, or an ordinary archive; those development/portable forms are not
update-managed.

The application reads the machine-only `LunCoSim/lunco-sim-updates` GitHub feed;
human-facing installers remain in the dated LunCoSim release.

## CLI Usage

```bash
target/debug/luncosim [FLAGS]
```

### Flags

`luncosim --help` prints this same list — it is generated from the flags the binary
actually parses, so prefer it if this table and the binary ever disagree.

| Flag | Description |
|---|---|
| `-h`, `--help` | Print usage and exit, without launching the simulator. |
| `--no-ui` | Run headless — no window, no GPU. Also via `LUNCO_NO_UI=1`. |
| `--headless-max-speed` | With `--no-ui` (or the headless server launcher), run the fixed simulation lattice as fast as the CPU and causal participants permit. This removes wall-clock pacing; it does not bypass the co-simulation barrier. |
| `--api [PORT]` | Enable the HTTP API server. Default port is 4101. **Not implied by `--no-ui`**: without this flag there is no API port at all. |
| `--scene <PATH>` | Load a specific USD scene. Path is relative to `assets/` (or may be workspace-relative/absolute). Without it, luncosim starts with an empty persistent world shell. |
| `--no-vsync` | Disable VSync. FPS will not be capped by the display refresh rate. |
| `--no-throttle` | Disable background throttling. The window will update at full rate even when unfocused. |
| `--log-diag` | Enable Bevy's `LogDiagnosticsPlugin` to print FPS, FrameTime, and physics stats to the console. |
| `--window-pos <SPEC>` | Force the OS window to a specific screen region (e.g., `1920x1080+0+0`). |
| `--host [PORT]` | Start a networked listen-server. Default port is 5888. |
| `--connect <ADDR>` | Connect to a networked server via WebTransport. An `ADDR` with no port implies `:5888`; a bare IP skips TLS validation (LAN/dev). |
| `--cert <PATH>` | TLS certificate for `--host`: a certbot live dir, or a cert file (then pair with `--key`). Omit both for a dev self-signed cert. See [OPS](OPS.md). |
| `--key <PATH>` | TLS private key, when `--cert` names a file rather than a directory. |

Measuring FPS? Always pass `--no-vsync --no-throttle` — otherwise you are timing
the compositor and the unfocused power-save throttle, not the renderer.

## Interactive Controls

The active controls are user-configurable through the `input_bindings` section
of `~/.lunco/settings.json`. The controller translates those bindings into
semantic intents, and the in-app help/input overlay displays the resolved labels.
Tutorials therefore describe actions such as forward, brake, thrust, cancel, or
release rather than embedding physical keys. Editing or rebinding a setting is
reflected by the live input map and by tutorial copy that uses
`input_binding(...)`/`input_hint(...)`.

Editor operations are available through the Command Deck and the relevant
context menus; their action names, not physical shortcut guesses, are the
stable documentation surface.

## See also

- [**USD Domain Architecture**](../../architecture/21-domain-usd.md) — how scenes are loaded and mapped to physics.
- [**Co-Simulation Domain**](../../architecture/22-domain-cosim.md) — how Modelica models and physics share a timestep.
- [**Cosim walkthrough**](../../tutorials/03-cosim.md) — build and observe a Modelica↔physics vessel.
- [**Attach a simulation program**](../../tutorials/04-attach-a-program.md) — use the Models palette or Rhai to author a source-backed USD program contract and verify its live ports.
- [**View & Intent Architecture**](../../architecture/17-view-and-intent.md) — how camera control and possession work.
