# Model Viewer

The **Model Viewer** is a minimal LunCoSim application for inspecting individual USD models and their hierarchical structure.

## What it does

- **Minimal inspection**: Loads a USD prim or stage with basic lighting.
- **API Enabled**: Implicitly serves the HTTP API, allowing external tools to query the model structure.
- **Rotation**: The viewer provides a simple orbit camera for inspection.

## CLI Usage

```bash
cargo run --bin model_viewer -- [FLAGS]
```

### Flags

| Flag | Description |
|---|---|
| `--api [PORT]` | Enable the HTTP API server. Default port is 4101 (enabled by default). |

## See also

- [**USD Domain Architecture**](../../architecture/21-domain-usd.md) — how the underlying USD pipeline works.
