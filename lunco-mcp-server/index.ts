import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import dotenv from "dotenv";

dotenv.config();

const SIM_URL = process.env.LUNCO_SIM_URL || "http://localhost:8082";

const server = new Server(
  {
    name: "lunco-mcp-server",
    version: "1.0.0",
  },
  {
    capabilities: {
      tools: {},
    },
  }
);

async function callSimApi(name: string, targetPath: string, args: Record<string, any>) {
  const response = await fetch(`${SIM_URL}/api/command`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, target_path: targetPath, arguments: args }),
  });
  if (!response.ok) throw new Error(`API call failed: ${response.statusText}`);
  return await response.json();
}

server.setRequestHandler(ListToolsRequestSchema, async () => {
  return {
    tools: [
      {
        name: "spawn_entity",
        description: "Spawn an entity in the LunCo simulation.",
        inputSchema: {
          type: "object",
          properties: {
            type: { type: "string", description: "Entity type (e.g., RoverRigid)" },
            position: { type: "array", items: { type: "number" }, description: "Optional [x, y, z]" }
          },
          required: ["type"],
        },
      },
      {
        name: "take_control",
        description: "Take control of an entity.",
        inputSchema: {
          type: "object",
          properties: { target: { type: "string", description: "Entity name/path" } },
          required: ["target"],
        },
      },
      {
        name: "list_entities",
        description: "List all entity types available for spawning.",
        inputSchema: { type: "object", properties: {} },
      },
      {
        name: "get_commands",
        description: "Get all available command definitions for the simulation.",
        inputSchema: { type: "object", properties: {} },
      },
    ],
  };
});

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  try {
    switch (request.params.name) {
      case "spawn_entity":
        return { content: [{ type: "text", text: JSON.stringify(await callSimApi("SPAWN", "Simulation", (request.params.arguments || {}) as Record<string, any>)) }] };
      case "take_control":
        return { content: [{ type: "text", text: JSON.stringify(await callSimApi("TAKE_CONTROL", "Avatar", (request.params.arguments || {}) as Record<string, any>)) }] };
      case "send_key":
        const { key, action } = (request.params.arguments || {}) as any;
        const cmd = action === "down" ? "KEY_DOWN" : "KEY_UP";
        return { content: [{ type: "text", text: JSON.stringify(await callSimApi(cmd, "Avatar", { key })) }] };
      case "list_entities":
        return { content: [{ type: "text", text: JSON.stringify(await callSimApi("LIST_ENTITIES", "Simulation", {})) }] };
      case "get_commands":
        const resp = await fetch(`${SIM_URL}/api/command`);
        return { content: [{ type: "text", text: JSON.stringify(await resp.json()) }] };
      default:
        throw new Error("Tool not found");
    }
  } catch (error: any) {
    return { content: [{ type: "text", text: `Error: ${error.message}` }], isError: true };
  }
});

async function run() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

run().catch(console.error);
