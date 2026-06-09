# CLI Reference

LunCoSim provides several command-line tools for running simulations, managing assets, and automation.

## 1. Primary Binaries

These binaries are the main entry points for the simulation and editor.

### `sandbox`
A standalone sandbox for rapid testing of ground mobility and physics.

**Usage:**
```bash
cargo run --bin sandbox -- [FLAGS]
```

**Flags:**
| Flag | Description |
|---|---|
| `--api [PORT]` | Enable the HTTP API server. Default port is 3000. |
| `--scene <PATH>` | Load a specific USD scene. Path is relative to `assets/`. Default: `scenes/sandbox/sandbox_scene.usda`. |
| `--no-vsync` | Disable VSync. FPS will not be capped by the display refresh rate. |
| `--no-throttle` | Disable background throttling. The window will update at full rate even when unfocused. |
| `--log-diag` | Enable Bevy's `LogDiagnosticsPlugin` to print FPS, FrameTime, and physics stats to the console. |
| `--window-pos <SPEC>` | Force the OS window to a specific screen region (e.g., `1920x1080+0+0`). |
| `--host [PORT]` | Start a networked listen-server. |
| `--connect <ADDR>` | Connect to a networked server via WebTransport. |

---

### `lunica`
The generic Modelica engineering workbench and IDE.

**Usage:**
```bash
cargo run --bin lunica -- [FLAGS]
```

**Flags:**
| Flag | Description |
|---|---|
| `--api [PORT]` | Enable the HTTP API server. Default port is 3000. |

---

### `model_viewer`
A minimal USD model viewer.

**Usage:**
```bash
cargo run --bin model_viewer -- [FLAGS]
```

**Flags:**
| Flag | Description |
|---|---|
| `--api [PORT]` | Enable the HTTP API server (implicitly enabled). |

---

## 2. Utility Binaries

### `lunco-assets`
CLI tool for managing LunCoSim assets (downloading, verifying, and listing).

**Usage:**
```bash
cargo run -p lunco-assets -- <ACTION> [FLAGS]
```

**Actions:**
| Action | Description |
|---|---|
| `download` | Download all workspace assets. |
| `list` | List all workspace assets. |
| `process` | Process downloaded assets (e.g., texture conversion). |

**Flags:**
| Flag | Description |
|---|---|
| `-p, --package <NAME>` | Target a specific crate (e.g., `lunco-modelica`). |
| `-a, --asset <KEY>` | Download/Process a single asset by its key from `Assets.toml`. |
| `--workspace-root <PATH>` | Override the workspace root directory. |
| `--help, -h` | Print usage information. |

---

### `net_smoke`
A headless network smoke test.

**Usage:**
```bash
cargo run -p lunco-networking --bin net_smoke -- [FLAGS]
```

---

## 3. Worker Binaries (Internal)

These are used as background processes by the primary binaries.

- **`lunica_worker`**: Background worker for rumoca Modelica compiles.
- **`msl_indexer`**: Utility for indexing the Modelica Standard Library (MSL).
