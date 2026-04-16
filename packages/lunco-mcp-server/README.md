# LunCoSim MCP Server

MCP server exposing LunCoSim HTTP API commands as MCP tools for AI agent integration.

## Installation

```bash
npm install
```

## Usage

```bash
# Start with defaults (localhost:3000)
npm start

# Or use npx directly from the package directory
npx .

# Custom host/port
npm start -- --host 192.168.1.100 --port 8080
```

## Available Tools

| Tool | Description |
|------|-------------|
| `execute_command` | Execute a LunCoSim command by name with optional parameters |
| `discover_schema` | Discover available commands and their parameters |
| `list_entities` | List all entities in the current simulation |
| `query_entity` | Get details about a specific entity by ID |

## Configuration for Claude Desktop / Cline

Add to your MCP settings:

```json
{
  "mcpServers": {
    "lunco": {
      "command": "npx",
      "args": ["/path/to/lunco-mcp-server/packages/lunco-mcp-server"]
    }
  }
}
```

Or install globally:

```bash
npm install -g @lunco/mcp-server
```
