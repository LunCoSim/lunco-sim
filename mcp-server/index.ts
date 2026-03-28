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

async function fetchAvailableCommands() {
  const response = await fetch(`${SIM_URL}/api/command`);
  if (!response.ok) throw new Error(`API call failed: ${response.statusText}`);
  return await response.json();
}

function mapCommandToTool(target: string, cmd: any) {
  const properties: Record<string, any> = {};
  const required: string[] = [];

  for (const arg of (cmd.arguments || [])) {
    const property: Record<string, any> = {
      type: arg.type === "vector3" ? "array" : arg.type === "enum" ? "string" : arg.type,
      description: arg.description || "",
    };
    if (arg.type === "vector3") {
      property.items = { type: "number" };
    }
    if (arg.type === "enum") {
      property.enum = arg.values;
    }
    properties[arg.name] = property;
    
    const isOptional = arg.default !== undefined || (arg.description && arg.description.toLowerCase().includes("optional"));
    if (!isOptional) {
      required.push(arg.name);
    }
  }

  const name = (target === "Simulation" || target === "Avatar") ? cmd.name : `${target}_${cmd.name}`;

  return {
    name: name,
    description: cmd.description || `${target} command: ${cmd.name}`,
    inputSchema: {
      type: "object",
      properties,
      required,
    },
  };
}

server.setRequestHandler(ListToolsRequestSchema, async () => {
  const { targets } = await fetchAvailableCommands();
  const tools = [];

  for (const target in targets) {
    for (const cmd of targets[target]) {
      tools.push(mapCommandToTool(target, cmd));
    }
  }

  return { tools };
});

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  try {
    let target: string;
    let cmdName: string;

    if (request.params.name === "SPAWN" || request.params.name === "DELETE" || request.params.name === "LIST_ENTITIES") {
      target = "Simulation";
      cmdName = request.params.name;
    } else if (["TAKE_CONTROL", "STOP_CONTROL", "KEY_DOWN", "KEY_UP", "KEY_PRESS"].includes(request.params.name)) {
      target = "Avatar";
      cmdName = request.params.name;
    } else {
      const [targetPart, ...cmdParts] = request.params.name.split("_");
      target = targetPart;
      cmdName = cmdParts.join("_");
    }
    
    const result = await callSimApi(cmdName, target, (request.params.arguments || {}) as Record<string, any>);
    return { content: [{ type: "text", text: JSON.stringify(result) }] };
  } catch (error: any) {
    return { content: [{ type: "text", text: `Error: ${error.message}` }], isError: true };
  }
});

async function run() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

run().catch(console.error);
