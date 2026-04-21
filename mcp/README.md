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
| `capture_screenshot` | Capture viewport as PNG |
| `execute_command` | Generic command executor |

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
