# LunCoSim MCP Server

MCP (Model Context Protocol) server for LunCoSim simulation control.

## Overview

This MCP server exposes LunCoSim simulation commands as MCP tools, enabling AI agents (Claude Desktop, Cline, Windsurf, etc.) to control and inspect the simulation programmatically.

## Features

- **Dynamic tool discovery** - Tools are generated from the simulation's API schema
- **Typed commands** - Each discovered command becomes a typed MCP tool
- **Resources** - Entities and simulation state exposed as MCP resources
- **Static tools** - Always-available tools for schema discovery, entity listing, and screenshots
- **Screenshot support** - Capture simulation viewport as PNG with proper MCP content types

## Installation

```bash
npm install -g @lunco/mcp-server
```

Or use directly with `npx`:

```bash
npx @lunco/mcp-server
```

## Configuration

Environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `LUNCO_API_HOST` | `localhost` | LunCoSim API host |
| `LUNCO_API_PORT` | `3000` | LunCoSim API port |

Example:

```bash
LUNCO_API_HOST=192.168.1.100 LUNCO_API_PORT=8080 npx @lunco/mcp-server
```

## Static Tools

These tools are always available:

| Tool | Description |
|------|-------------|
| `discover_schema` | Get all available commands and parameters |
| `list_entities` | List all simulation entities |
| `query_entity` | Get entity details by ID |
| `capture_screenshot` | Capture viewport as PNG (optionally save to file) |
| `execute_command` | Generic command executor |
| `list_bundled` | List embedded `assets/models/*.mo` example models with `bundled://` URIs |
| `list_open_documents` | List every open document (Modelica / USD / SysML / future kinds) with origin + active flag |
| `list_twin` | List files in the open Twin folder, paginated, classified by kind |
| `list_msl` | Paginated, filterable enumeration of Modelica Standard Library classes |
| `open_uri` | Unified scheme-aware open (`bundled://`, `mem://`, qualified MSL name, fs path) |
| `compile_model` | Compile an open document, optionally targeting a specific class (bypasses GUI picker) |
| `compile_status` | Read per-doc compile state without triggering compile |
| `list_compile_candidates` | List the non-package classes a multi-class doc would let you compile |
| `get_document_source` | Fetch the in-memory source of an open doc (incl. unsaved edits) |
| `describe_model` | Full structural view of a class: `class_kind`, `extends`, `components`, `connections`, plus typed `inputs / parameters / outputs` with units & bounds |
| `snapshot_variables` | One-shot read of current parameter / input / variable values from a running sim |
| `set_input` | Push a runtime input value into a compiled model. Returns `{ok}` or structured error listing known input names |
| `find_model` | Fuzzy search across bundled / Twin / MSL / open docs. Returns ranked URIs with relevance scores |

### Edit API

Mutation commands are reachable via `execute_command`:

| Command | Purpose |
|---|---|
| `SetDocumentSource` | Replace an open document's full source text |
| `AddModelicaComponent` | Add a sub-component to a class (`AddComponent` AST op) |
| `RemoveModelicaComponent` | Remove a sub-component |
| `ConnectComponents` | Add a `connect(a.p, b.q)` equation |
| `DisconnectComponents` | Remove a connect equation |
| `ApplyModelicaOps` | **Batched** — apply N ops (`Add/RemoveComponent`, `Add/RemoveConnection`, `SetPlacement`, `SetParameter`) in a single observer pass. The same Reflect event the canvas drag-drop pipeline fires — agents and the GUI share one path. Each op still produces an independent undo entry today; transactional grouping is a follow-up. |

Every op flows through the same `ModelicaOp` undo/redo pipeline the
canvas drag-and-drop uses, so mutations are undoable and journaled
identically to UI-driven edits.

The listing tools (`list_*`, `msl_status`) are introduced in spec
[`032-model-source-listing`](../specs/032-model-source-listing/spec.md).
They use a generic `ApiQueryProvider` extension point in `lunco-api`, so
domain crates register their own listings without `lunco-api` taking a
direct dep on them.

### Example: end-to-end agent workflow

The combination above is designed so an agent can run:

```
1. find / list                  list_bundled  →  pick "AnnotatedRocketStage.mo"
2. open                         open_uri(uri="bundled://AnnotatedRocketStage.mo")
3. compile + run                execute_command(CompileActiveModel, …) +
                                execute_command(ResumeActiveModel, …)
4. inspect what's running       list_open_documents
5. tweak a value, observe       (covered by spec 033 — describe_model,
                                 set_input, snapshot_variables)
```

…without touching the GUI.

A runnable smoke test of this exact workflow lives at
`tests/api/agent_workflow.sh` — start the workbench with
`--api 3000` and run the script to verify every endpoint above
end-to-end against AnnotatedRocketStage.

## Resources

These resources provide declarative access to simulation state:

| Resource URI | Description |
|--------------|-------------|
| `lunco://entities` | List of all entities in the simulation (JSON) |
| `lunco://entities/{id}` | Detailed state of a specific entity (JSON) |

## Prompts

Predefined prompts to help agents interact with the simulation:

| Prompt Name | Description | Arguments |
|-------------|-------------|-----------|
| `rover_status` | Comprehensive status report for a rover | `target` (entity ID) |
| `mission_briefing` | Summary of the current simulation state | (none) |

## Dynamic Tools

Commands discovered from the simulation's schema are exposed as typed tools:

- `drive_rover` - Drive a rover (target, forward, steer)
- `brake_rover` - Apply brakes (target, intensity)
- `possess_vessel` - Possess a vessel (avatar, target)
- `release_vessel` - Release vessel (target)
- `focus_target` - Focus camera on target (avatar, target)
- `teleport_to_surface` - Teleport to surface (target, body_entity)
- `leave_surface` - Return to orbit (target)
- `spawn_entity` - Spawn from catalog (target, entry_id, position)
- `open_example` - Open MSL class (qualified)
- `auto_arrange_diagram` - Layout diagram (doc)
- `set_view_mode` - Switch view mode (doc, mode)
- `set_zoom` - Set zoom level (doc, zoom)
- `fit_canvas` - Fit diagram in view (doc)
- `pan_canvas` - Pan viewport (doc, x, y)
- `move_component` - Move component (class, name, x, y, width, height)
- `undo` - Undo last edit (doc)
- `redo` - Redo edit (doc)
- `exit` - Exit simulation
- `get_file` - Read file contents (path)

## IDE Integration

### AI agents (this repo)

This repo ships a project-scoped `.mcp.json` at the workspace root that
runs the server straight from `mcp/src/index.js`. On first launch it
auto-installs dependencies (`npm install` runs only if `mcp/node_modules/`
is missing), then execs the server.

`.mcp.json` is read by any MCP-aware agent that supports project-scoped
config (Claude Code, Cline, Cursor, Windsurf, etc.). The agent will
typically prompt you once to approve the server before it loads.

No global install needed — just open the repo in your agent of choice.

### Claude Desktop

Add to `~/.claude/mcp.json`:

```json
{
  "mcpServers": {
    "lunco": {
      "command": "npx",
      "args": ["@lunco/mcp-server"],
      "env": {
        "LUNCO_API_HOST": "localhost",
        "LUNCO_API_PORT": "3000"
      }
    }
  }
}
```

### Cline

Add to VS Code settings or `.vscode/mcp.json`:

```json
{
  "mcpServers": {
    "lunco": {
      "command": "npx",
      "args": ["@lunco/mcp-server"]
    }
  }
}
```

## Protocol

The server uses stdio transport with the MCP protocol. Communication flow:

```
AI Agent <-> MCP Client <-> lunco-mcp-server <-> LunCoSim API <-> Simulation
```

All API calls go through `POST /api/commands` with a unified request envelope.

## Development

```bash
cd mcp
npm install
npm run dev  # Watch mode
npm start     # Production
```

## License

MIT
