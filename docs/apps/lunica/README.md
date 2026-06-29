# Lunica

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

### 1. MSL Bootstrap
Lunica needs the Modelica Standard Library (MSL) on hand. On first use (desktop):
1. **Download MSL**: Ensure MSL sources are in `~/.cache/lunco/msl/`.
2. **Index MSL**: Run the indexer to produce the pre-parsed cache:
   ```bash
   cargo run --release -p lunco-modelica --bin msl_indexer
   ```

### 2. Modeling & Simulation
- **Edit**: Use the text or schematic editor to build models.
- **Compile**: Click "Compile" to generate the DAE.
- **Simulate**: Configure experiment bounds and run the solver.
- **Analyze**: Plot variables in real-time or from results.

## See also

- [**Modelica Domain Architecture**](../../architecture/20-domain-modelica.md) — how Lunica handles DAEs and solvers.
- [**Wasm Web Worker & Web Build**](../../architecture/30-wasm-web-worker.md) — the wasm/WebGPU pipeline, build/deploy, and how Lunica stays responsive during compiles on the web.
