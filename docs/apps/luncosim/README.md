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

## Perspectives

Choose a task from the title-bar tabs or **View → Perspectives**:

- **View** operates the live mission in a full-window scene.
- **Build** composes the scene from assemblies, with Entities, Twin sources,
  Inspector, Environment and Spawn.
- **Analyze** compares signals in central Graphs, with Telemetry and Ports.
- **Editor** edits one explicit USD document in an isolated preview.
- **Lunica** opens Modelica diagrams, equations, compilation and experiments.

Optional panels live in the corresponding **View** submenus. The **Help →
LunCoSim documentation** link opens the [published guide](https://luncosim.github.io/luncosim-docs/#current-status).
**Help → Perspective help** explains the selected workflow; **View → Reset
Layout** restores its authored default. Switching tasks preserves the Twin and
simulation.

Perspectives are assembled by Rhai over the installed capabilities. A Twin may
ship its own task presets; follow [Twin perspectives](../../tutorials/05-twin-perspectives.md).
The [workbench architecture](../../architecture/11-workbench.md#4-perspectives)
defines composition, validation and personal layout persistence.

### Finite-world escape containment

Dynamic bodies can fly beyond bounds inferred from static colliders by
default. To pause an object when it leaves those finite bounds, enable
**Contain bodies outside finite physics worlds** under **Settings -> General**.
Use **Settings -> Project** to set a local override, or **Twin settings** to
save a value shared with everyone using that Twin. Twin overrides project;
project overrides the system default. Non-finite physics state is always
contained, regardless of these settings.

The local project file is `<project>/.lunco/settings.json`:

```json
{ "physics": { "finite_world_escape_guard": true } }
```

The shared Twin setting is authored in `twin.toml`:

```toml
[settings]
"physics.finite_world_escape_guard" = true
```

The `GetPhysicsEscapeGuardSettings` query reports the effective value and its
source. See the [Twin workflow guide](../../architecture/13-twin-and-workflow.md#finite-world-physics-escape-containment).

### Open a Twin without its default scene

LunCoSim normally loads the default scene declared by a Twin. To change the
app-wide default, choose **Settings → General** and toggle **Load the default
scene when opening a Twin**. This checkbox is available without an open Twin.

To change one project, open it and choose **Settings → Project → Default scene
on open**. Select **Use app setting**, **Load default scene**, or **Start
empty**. This choice is local to the project folder. The menu reports
when a shared Twin setting or command-line flag takes precedence; follow
**Edit Twin setting…** to change the shared value saved in `twin.toml`.
To set a shared value for the first time, open **View → Lunica → Twin settings**.
Reopen the Twin with **File → Open Folder/Twin…** to apply changes.

When the Twin opens without a scene, the viewport lists its configured scenes
and offers **Load default scene** if the declared file exists. Choosing a scene
starts it for the current session without changing the saved loading preference
or the Twin's default scene. If there are no scenes, the viewport says so.

A folder without its own `twin.toml` is a temporary workspace: no Twin or
project is open. The viewport lists USD files directly in that folder, or says
that the folder has no scenes to load. Open standalone USD, Modelica, Rhai, and other supported
files in the Editor. Project and shared Twin settings do not apply to this
workspace; LunCoSim does not look for Twins inside the folder.

To use the Editor with no default scene, select **Editor** in the title bar.
Open a USD file from the Twin browser or **File → Open File…**, or create one
with **File → New → USD Stage**. The document opens in an isolated preview;
edit and save it normally. Opening a document inside the active Twin does not
load it into the live simulation or change the Twin's authored default scene.

You can also edit the project file `<project>/.lunco/settings.json` before
opening the Twin:

```json
{ "scene": { "load_default": false } }
```

Remove `load_default` or use `null` to inherit the global preference. The
global preference uses the same JSON section in `lunco/settings.json` under the
OS configuration directory: normally `~/.config/lunco/` on Linux,
`~/Library/Application Support/lunco/` on macOS, and `%APPDATA%\lunco\` on
Windows. `LUNCOSIM_CONFIG` replaces that directory. Edit the global file while
the application is closed, or use the checkbox in the running application.

For a shared Twin override, use this in `twin.toml`:

```toml
[settings]
"scene.load_default" = false
```

Remove the key to inherit the project setting. The
[Twin workflow guide](../../architecture/13-twin-and-workflow.md#default-scene-loading)
also documents programmatic access and error handling.

### Desktop updates

The official installers are platform-specific:

| Platform | Download | Launch after installation |
|---|---|---|
| Windows x86_64 | `LunCoSim-Windows-x86_64.msi` | The installed Start-menu or desktop shortcut |
| macOS Apple Silicon | `LunCoSim-macOS-Apple-Silicon.pkg` | The installed `LunCoSim.app` |
| macOS Intel | `LunCoSim-macOS-Intel.pkg` | The installed `LunCoSim.app` |
| Linux x86_64 | `LunCoSim-Linux-x86_64.AppImage` | The same AppImage from a writable location |

On Linux, starting the AppImage adds a per-user application-menu entry and icon
so desktop environments, including Wayland, can identify its window. The entry
follows the AppImage if you move and launch it again. An existing unrelated
`luncosim.desktop` is left untouched. After deleting the AppImage, remove
`~/.local/share/applications/luncosim.desktop` and
`~/.local/share/icons/hicolor/256x256/apps/io.github.luncosim.AppImage.png` to
remove this integration; use `$XDG_DATA_HOME` instead of `~/.local/share` if set.

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
than the installer, a copied `.app` binary, `target/debug/luncosim`, a source
build, or an ordinary archive; those development/portable forms are not
update-managed.

The application reads update feeds and downloads update packages from
`LunCoSim/lunco-sim-updates`. Unsigned nightly installers are published in that
repository's nightly releases. Signed Dev and stable installers are published
in the `LunCoSim/lunco-sim` releases. See the
[code-signing policy](../../code-signing-policy.md) for Windows release signing.

The Windows installer has an internal, ordered MSI version distinct from the
displayed application version. Development SemVer versions such as
`0.6.0-dev.1` map to `1.6.1`; a stable `0.6.0` maps to `1.6.999`. Publication
rejects non-increasing versions within the development SemVer train and keeps
the MSI number ahead of every published Windows package. Windows Installer
compares only the three MSI version fields.

The first `0.6.0-dev.1` release sorts before existing `0.6.0-nightly.*` builds
under SemVer. Install its MSI manually to move an existing nightly installation
to the Dev update sequence; later Dev builds can update through Velopack.

The native package build enables the updater explicitly. Ordinary source and
headless builds omit the updater closure and are not update-managed.

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
| `--scene <PATH>` | Load a specific USD scene from a relative or absolute filesystem path. Startup resolves its owning root and mounts it through the canonical `twin://` address, and opens the simulator in the **View** perspective even if that Twin was last left in Design/Lunica. Without it, luncosim starts with an empty persistent world shell. |
| `--no-load-default-scene` | Disable automatic default-scene loading for Twins opened in this run. An explicit `--scene` still opens. |
| `--load-default-scene` | Enable automatic default-scene loading for Twins opened in this run. Overrides saved preferences. |
| `--render-profile <standard\|fast>` | Select the startup compatibility profile. `fast` uses unlit, texture-free materials and disables HDR, bloom, and MSAA. |
| `--render-quality <low\|balanced\|high>` | Override the Graphics quality for this process. Fresh settings default to `high`; choose `low` for lower-end machines. `high` is the highest shipped visual preset, raising shadow, sky-cubemap, lunar-terrain, LOD, and tessellation budgets. |
| `--record-offline <dir\|out.mp4>` | Arm a GPU-backed PNG sequence or direct video recording; offscreen captures begin after the authored scene is presentable. |
| `--record-fps <N>` | Set the recorded output rate; requires `--record-offline`. Defaults to 60. |
| `--record-frames <N>` | Stop a one-shot recording after `N` frames; requires `--record-offline`. |
| `--record-size <WxH>` | Set the offscreen render target size. Defaults to 1280x720. |
| `--offscreen` | Use the GPU-backed windowless capture path; combine with `--record-offline` for a one-shot take. |
| `--no-vsync` | Disable VSync. FPS will not be capped by the display refresh rate. |
| `--no-throttle` | Disable background throttling. The window will update at full rate even when unfocused. |
| `--log-diag` | Enable Bevy's `LogDiagnosticsPlugin` to print FPS, FrameTime, and physics stats to the console. |
| `--console` | Windows desktop builds: enable the debug console for this launch without changing the saved preference. |
| `--window-pos <SPEC>` | Force the OS window to a specific screen region (e.g., `1920x1080+0+0`). |
| `--host [PORT]` | Start a networked listen-server. Default port is 5888. |
| `--connect <ADDR>` | Connect to a networked server via WebTransport. An `ADDR` with no port implies `:5888`; a bare IP skips TLS validation (LAN/dev). |
| `--cert <PATH>` | TLS certificate for `--host`: a certbot live dir, or a cert file (then pair with `--key`). Omit both for a dev self-signed cert. See [OPS](OPS.md). |
| `--key <PATH>` | TLS private key, when `--cert` names a file rather than a directory. |

Measuring FPS? Always pass `--no-vsync --no-throttle` — otherwise you are timing
the compositor and the unfocused power-save throttle, not the renderer.

### Windows debug console

Windows desktop launches open the simulator without a console by default.
Enable **Settings → Debug → Show console on next start**, then restart, to
see startup logs. The preference stays enabled until unchecked; it does not
open or close a console in the current session. It is stored as
`debug_console.show_on_startup` in the shared `lunco/settings.json` document.
To return to startup without a console, uncheck the same setting and restart.
On Windows the settings file normally lives at
`%APPDATA%\lunco\settings.json`; when `LUNCOSIM_CONFIG` is set, it lives in
that directory instead. Other user preferences share this file.

If startup fails before you can open Settings, open PowerShell in the directory
containing `luncosim.exe` and launch it with:

```powershell
.\luncosim.exe --console
```

This enables the console for one launch without changing the saved preference.

Launching from a terminal attaches to that terminal, and inherited redirected
streams remain usable for CLI commands and captured logs. Builds without the
`ui` feature and `luncosim-server` retain their console subsystem.

On Windows, verify startup, preference persistence across restarts, the
one-launch override, and redirected CLI output with the production binary:

```powershell
$env:LUNCOSIM_BIN = (Resolve-Path target/debug/luncosim.exe).Path
python scripts/api/test_windows_console.py --port 4127
```

### GPU-backed visual tests

`luncosim test` is intentionally headless and therefore does not render visuals.
For a Summer Space School visual smoke test or capture, run the same production
binary through the offscreen GPU path and select the explicit highest-quality
preset:

```bash
target/debug/luncosim --api 4107 --offscreen --render-quality high \
  --record-offline ~/.lunco/cache/summer-space-school/high-quality \
  --record-frames 3 --record-size 1280x720 \
  --scene /path/to/space-school-twin/sim/scenes/traverse.usda
```

The requested API port must not already be in use; otherwise startup exits
before the simulator window and loop are started. The quality preset controls
renderer budgets, including
shadows, the sky cubemap, lunar terrain LOD and tessellation; USD remains the
owner of the scene's authored shader sources (such as the Summer Space School
terrain and starfield shaders). The local graphics scene gate uses this preset by
default; set `RENDER_QUALITY=balanced` when a lower-budget comparison is needed.

## Interactive Controls

The active controls are user-configurable through the `input_bindings` section
of `<OS config dir>/lunco/settings.json`. The controller translates those bindings into
semantic intents, and the in-app help/input overlay displays the resolved labels.
Tutorials therefore describe actions such as forward, brake, thrust, cancel,
release, or speed boost rather than embedding physical keys. Editing or rebinding a setting is
reflected by the live input map and by tutorial copy that uses
`input_binding(...)`/`input_hint(...)`.

For the free-flight avatar, speed boost is a semantic `SpeedBoost` intent and
an authored normalized `speed_boost` command port. It is delivered in the same
controller command frame as movement, including while Q/E transitions.

When a lander is possessed, Telemetry shows its authored body-axis mapping and
the selected camera frame. In the default `orbit` frame, forward/back, left/right,
and yaw remain lander-relative even when the camera is moved; opposite commands
cancel and release clears the authored command ports.

Editor operations are available through the Command Deck and the relevant
context menus; their action names, not physical shortcut guesses, are the
stable documentation surface.

## See also

- [**USD Domain Architecture**](../../architecture/21-domain-usd.md) — how scenes are loaded and mapped to physics.
- [**Co-Simulation Domain**](../../architecture/22-domain-cosim.md) — how Modelica models and physics share a timestep.
- [**Cosim walkthrough**](../../tutorials/03-cosim.md) — build and observe a Modelica↔physics vessel.
- [**Attach a simulation program**](../../tutorials/04-attach-a-program.md) — use the Models palette or Rhai to author a source-backed USD program contract and verify its live ports.
- [**View & Intent Architecture**](../../architecture/17-view-and-intent.md) — how camera control and possession work.
