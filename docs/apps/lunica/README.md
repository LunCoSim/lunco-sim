# Lunica

> **Try it live:** [**lunica.lunco.space**](https://lunica.lunco.space) — runs in the browser. Early preview build; expect rough edges.

**Lunica** is the Modelica-focused subset of LunCoSim. It is a specialized workbench for engineering modeling, simulation, and analysis, focused entirely on the Modelica domain.

## What it is

- **Focused Workbench**: Ships only the Modelica modelling/simulation experience: code editor, schematic diagram, package browser, simulator, and plots.
- **Cross-Platform**: Compiles to a single desktop binary (native) **and** to WebAssembly for the browser.
- **Rumoca-Powered**: Uses the `rumoca` engine for parsing, DAE generation, and simulation.

## CLI Usage (Desktop)

```bash
cargo run --bin lunica -- [FLAGS]
```

### Flags

| Flag | Description |
|---|---|
| `--api [PORT]` | Enable the HTTP API server. Default port is 4101. |

## Web Usage (Wasm)

Lunica can be served as a web application:

```bash
./scripts/build_web.sh build lunica
./scripts/build_web.sh serve lunica   # http://localhost:8080
```

## Key Workflows

### 1. Modelica source libraries
No external Modelica library is required to launch Lunica. The Twin Browser and
Components palette use the package roots and classes present in the installed
library index, so any installed library can be browsed through the same UI.
Curated Welcome learning paths appear only when all their referenced classes
are present; indexed library examples and bundled LunCoSim demos remain
available when no curated path matches.

On desktop, install a source-library dataset through **Settings → Data &
libraries**, or configure a local source root through **Settings → Modelica**.
The workbench builds the editor index in the background; it can also be
generated directly for a configured source root to produce the pre-parsed
cache:
   ```bash
   cargo run --release -p lunco-modelica-assets --bin modelica_library_indexer
   ```

### 2. Modeling & Simulation
- **Edit**: Use the text or schematic editor to build models.
- **Compile**: Click "Compile" to generate the DAE.
- **Simulate**: Configure experiment bounds and run the solver.
- **Analyze**: Plot variables in real-time or from results.

The Modelica Workspace remains pinned in the center. Closing the last model
returns to its start actions while the Twin Browser, Components palette and
Inspector remain in their assigned panes.

## See also

- [**Modelica Domain Architecture**](../../architecture/20-domain-modelica.md) — how Lunica handles DAEs and solvers.
- [**Wasm Web Worker & Web Build**](../../architecture/30-wasm-web-worker.md) — the wasm/WebGPU pipeline, build/deploy, and how Lunica stays responsive during compiles on the web.
