# LunCo MCP Server Guide

This guide explains how to use and extend the dynamic LunCo MCP server.

## Overview
The LunCo MCP Server acts as a bridge between AI agents and the simulation environment. It dynamically discovers and exposes simulation commands as MCP tools, meaning **new commands added to the simulation are automatically available as MCP tools without modifying the server code.**

## How It Works
1.  **Dynamic Discovery**: When a client requests the list of tools, the server fetches command metadata from `GET /api/command` on the simulation server (default: `http://localhost:8082`).
2.  **Tool Mapping**: The server transforms API command definitions into MCP tool schemas.
    *   **Naming Convention**: Tools are exposed as `Target_Command` (e.g., `Rover_SET_MOTOR`, `Simulation_SPAWN`).
    *   **Argument Validation**: Arguments, enums, and defaults from the simulation API are automatically mapped to JSON Schema properties.
3.  **Command Proxy**: When a tool is called, the server parses the tool name back into a `target` and `command`, and proxies the arguments to `POST /api/command`.

## Configuration
The server environment is configured via `lunco-mcp-server/mcp-config.json` and standard environment variables:

- `LUNCO_SIM_URL`: The base URL of the running simulation (e.g., `http://localhost:8082`).

## Adding New Features
Because the server is dynamic:
- **Add a command in the simulation**: Simply define the new command in the Godot simulation's command handler.
- **Update MCP**: Restart the MCP server (or refresh the tool list in your client). The new tool will appear automatically.

## Debugging
- **Check Simulation API**: You can verify the raw commands known to the simulation by running:
  ```bash
  curl http://localhost:8082/api/command | jq .
  ```
- **Check Server Logs**: If a command is missing or failing, ensure the `LUNCO_SIM_URL` matches your running simulation, and that the simulation is correctly reporting the new command in its API output.
- **MCP Client**: Most MCP clients (like Claude Desktop or Cursor) automatically refresh tool lists on initialization. If changes aren't appearing, restart your client application.
