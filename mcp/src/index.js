#!/usr/bin/env node
/**
 * LunCoSim MCP Server
 *
 * Exposes LunCoSim simulation commands as MCP tools for AI agent integration.
 * Dynamic tools are generated from the API schema on startup.
 *
 * Usage:
 *   npx @lunco/mcp-server                    # Use default localhost:3000
 *   LUNCO_API_HOST=192.168.1.100 LUNCO_API_PORT=8080 npx @lunco/mcp-server
 */

import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  ListResourcesRequestSchema,
  ReadResourceRequestSchema,
  ListPromptsRequestSchema,
  GetPromptRequestSchema,
} from '@modelcontextprotocol/sdk/types.js';
import { writeFile, mkdir } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import {
  apiRequest,
  executeCommand,
  discoverSchema,
  listEntities,
  queryEntity,
  captureScreenshot,
} from './api.js';

const SERVER_NAME = 'lunco-mcp-server';
const SERVER_VERSION = '0.3.0';

// MCP Server instance
const server = new Server(
  {
    name: SERVER_NAME,
    version: SERVER_VERSION,
  },
  {
    capabilities: {
      tools: {},
      resources: {},
      prompts: {},
    },
  }
);

// Static tools (always available)
const STATIC_TOOLS = [
  {
    name: 'discover_schema',
    description: 'Discover all available LunCoSim commands and their parameters. Call this to get an up-to-date list of all commands the simulation supports.',
    inputSchema: {
      type: 'object',
      properties: {},
    },
  },
  {
    name: 'list_entities',
    description: 'List all entities currently in the simulation (rovers, planets, vessels, etc.). Returns entity IDs, names, and types.',
    inputSchema: {
      type: 'object',
      properties: {},
    },
  },
  {
    name: 'query_entity',
    description: 'Get detailed information about a specific entity by its API ID.',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'string',
          description: 'The entity ID (ULID format, e.g. "01ARNS3G...")',
        },
      },
      required: ['id'],
    },
  },
  {
    name: 'capture_screenshot',
    description: 'Capture a screenshot of the current simulation view. By default returns the PNG as an inline image. If `path` is given, writes the PNG to that file (resolved against the server cwd) and returns the absolute path instead. If `return_base64` is true, also returns the raw base64 string as text content (useful for piping to a file via the agent).',
    inputSchema: {
      type: 'object',
      properties: {
        path: {
          type: 'string',
          description: 'Optional file path to save the PNG to. Relative paths resolve against the server cwd. Parent directories are created automatically.',
        },
        return_base64: {
          type: 'boolean',
          description: 'If true, return the PNG bytes as a base64 text block in addition to the inline image. Default false.',
          default: false,
        },
      },
    },
  },
  {
    name: 'execute_command',
    description: 'Execute any LunCoSim command by name with JSON parameters. Use this as a fallback for commands not yet registered as typed tools, or for dynamic command execution.',
    inputSchema: {
      type: 'object',
      properties: {
        command: {
          type: 'string',
          description: 'The command name (e.g. "DriveRover", "SpawnEntity")',
        },
        params: {
          type: 'object',
          description: 'Command parameters as a JSON object',
          default: {},
        },
      },
      required: ['command'],
    },
  },
  // ── Model source listing (spec 032) ────────────────────────────────────
  {
    name: 'list_bundled',
    description: 'List embedded LunCoSim example models (assets/models/*.mo). Returns each entry with `filename`, `tagline`, and a `bundled://` URI suitable for `open_uri`. Always reachable, no pagination.',
    inputSchema: { type: 'object', properties: {} },
  },
  {
    name: 'list_open_documents',
    description: 'List every document currently open in the workspace — Modelica, USD, SysML, mission, or any future kind. Returns `doc_id`, `title`, `kind`, `origin` (file vs untitled), and `active` flag. Use this to decide whether to focus an existing tab or open something new.',
    inputSchema: { type: 'object', properties: {} },
  },
  {
    name: 'list_twin',
    description: 'List files in the currently-open Twin folder. Returns `{open: false}` when no Twin is open. Otherwise returns `{root, files, total}` with each file carrying a relative path, absolute path, and kind label (e.g. `document/modelica`, `file_reference`). Supports pagination via `limit` + `offset`.',
    inputSchema: {
      type: 'object',
      properties: {
        limit: { type: 'integer', description: 'Max files to return (omit for all).' },
        offset: { type: 'integer', description: 'Index of the first file to return (default 0).' },
      },
    },
  },
  {
    name: 'msl_status',
    description: 'Check whether the Modelica Standard Library has finished its background prewarm. Returns `{loaded, class_count, examples_count}`. Cheap and non-blocking — call this before `list_msl` if you want to avoid the cold-start parse cost.',
    inputSchema: { type: 'object', properties: {} },
  },
  {
    name: 'list_msl',
    description: 'List MSL classes with cursor-based pagination and filters. The MSL has ~2500 entries — use filters to narrow. Returns `{items, count, total_matched, next_cursor}`. Pass `next_cursor` back as `cursor` for the next page.',
    inputSchema: {
      type: 'object',
      properties: {
        cursor: { type: 'string', description: 'Cursor token returned by the previous page (decimal-string offset). Omit on first call.' },
        limit: { type: 'integer', description: 'Max items per page (default 200, max 1000).' },
        filter: {
          type: 'object',
          description: 'Optional filters. Combined with AND.',
          properties: {
            prefix: { type: 'string', description: 'Qualified-name prefix, e.g. "Modelica.Blocks".' },
            category: { type: 'string', description: 'Top-level package, e.g. "Blocks", "Electrical".' },
            examples_only: { type: 'boolean', description: 'When true, return only `*.Examples.*` classes.' },
          },
        },
      },
    },
  },
  {
    name: 'open_uri',
    description: 'Unified open command — dispatches on the URI scheme. Accepts `bundled://Filename.mo` (embedded example), `mem://Name` (re-focus existing Untitled tab), a dot-separated qualified Modelica name (`Modelica.Blocks.Examples.PID_Controller`, opened as MSL example), or a filesystem path. Use this in preference to `OpenFile`/`OpenClass`/`OpenExample`.',
    inputSchema: {
      type: 'object',
      properties: {
        uri: { type: 'string', description: 'The URI to open.' },
      },
      required: ['uri'],
    },
  },
];

// Dynamic tools cache (built from schema)
let DYNAMIC_TOOLS = [];
let LAST_SCHEMA_HASH = null;

/**
 * Build MCP tools from the discovered command schema.
 * @param {Object} schema - API schema from discover_schema
 * @returns {Array} Array of MCP tool definitions
 */
function buildDynamicTools(schema) {
  if (!schema || !schema.commands) {
    return [];
  }

  // Filter out commands that are already handled by static tools with special logic
  const EXCLUDED_COMMANDS = ['CaptureScreenshot'];
  const commands = schema.commands.filter(cmd => !EXCLUDED_COMMANDS.includes(cmd.name));

  return commands.map((cmd) => {
    // Build input schema from fields
    const properties = {};
    const required = [];

    for (const field of cmd.fields || []) {
      let jsonSchema = { type: 'string' };
      if (field.type_name.includes('f64') || field.type_name.includes('f32')) {
        jsonSchema = { type: 'number' };
      } else if (field.type_name.includes('i64') || field.type_name.includes('i32') || field.type_name.includes('u64') || field.type_name.includes('u32')) {
        jsonSchema = { type: 'integer' };
      } else if (field.type_name.includes('bool')) {
        jsonSchema = { type: 'boolean' };
      } else if (field.type_name.includes('Vec3')) {
        jsonSchema = {
          type: 'object',
          properties: {
            x: { type: 'number' },
            y: { type: 'number' },
            z: { type: 'number' },
          },
          required: ['x', 'y', 'z'],
        };
      } else if (field.type_name.includes('Entity')) {
        jsonSchema = { type: 'string', description: 'ULID entity identifier' };
      }

      properties[field.name] = {
        ...jsonSchema,
        description: `Parameter: ${field.name} (${field.type_name})`,
      };
      required.push(field.name);
    }

    // Convert command name to tool name (CamelCase -> snake_case)
    const toolName = cmd.name
      .replace(/([A-Z])/g, '_$1')
      .toLowerCase()
      .replace(/^_/, '')
      .replace(/_+/g, '_');

    return {
      name: toolName,
      description: `Execute LunCoSim command: ${cmd.name}`,
      inputSchema: {
        type: 'object',
        properties,
        required,
      },
    };
  });
}

/**
 * Hash the schema for change detection.
 */
function hashSchema(schema) {
  return JSON.stringify(schema?.commands || []);
}

/**
 * Fetch and cache schema, rebuild tools if changed.
 */
async function refreshSchema() {
  try {
    const result = await discoverSchema();
    const schema = result.data;
    const hash = hashSchema(schema);

    if (hash !== LAST_SCHEMA_HASH) {
      DYNAMIC_TOOLS = buildDynamicTools(schema);
      LAST_SCHEMA_HASH = hash;
      console.error(`[LunCoSim MCP] Schema updated: ${schema.commands?.length || 0} commands available`);
    }
  } catch (error) {
    console.error(`[LunCoSim MCP] Failed to fetch schema: ${error.message}`);
  }
}

/**
 * Get all tools (static + dynamic).
 */
function getAllTools() {
  return [...STATIC_TOOLS, ...DYNAMIC_TOOLS];
}

// Handle tool listing
server.setRequestHandler(ListToolsRequestSchema, async () => {
  return { tools: getAllTools() };
});

// Handle resource listing
server.setRequestHandler(ListResourcesRequestSchema, async () => {
  return {
    resources: [
      {
        uri: 'lunco://entities',
        name: 'Simulation Entities',
        description: 'A list of all entities currently in the simulation',
        mimeType: 'application/json',
      },
    ],
  };
});

// Handle resource reading
server.setRequestHandler(ReadResourceRequestSchema, async (request) => {
  const uri = request.params.uri;

  if (uri === 'lunco://entities') {
    const result = await listEntities();
    return {
      contents: [
        {
          uri,
          mimeType: 'application/json',
          text: JSON.stringify(result, null, 2),
        },
      ],
    };
  }

  if (uri.startsWith('lunco://entities/')) {
    const id = uri.split('/').pop();
    const result = await queryEntity(id);
    return {
      contents: [
        {
          uri,
          mimeType: 'application/json',
          text: JSON.stringify(result, null, 2),
        },
      ],
    };
  }

  throw new Error(`Resource not found: ${uri}`);
});

// Handle prompt listing
server.setRequestHandler(ListPromptsRequestSchema, async () => {
  return {
    prompts: [
      {
        name: 'rover_status',
        description: 'Get a comprehensive status report for a rover',
        arguments: [
          {
            name: 'target',
            description: 'The entity ID of the rover to inspect',
            required: true,
          },
        ],
      },
      {
        name: 'mission_briefing',
        description: 'Get a summary of the current simulation state and entities',
      },
    ],
  };
});

// Handle prompt retrieval
server.setRequestHandler(GetPromptRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;

  if (name === 'rover_status') {
    const target = args.target;
    return {
      messages: [
        {
          role: 'user',
          content: {
            type: 'text',
            text: `Please provide a detailed status report for the rover with ID ${target}. Include its current position, components, and any active telemetry. You can use the 'query_entity' tool or read the 'lunco://entities/${target}' resource.`,
          },
        },
      ],
    };
  }

  if (name === 'mission_briefing') {
    return {
      messages: [
        {
          role: 'user',
          content: {
            type: 'text',
            text: "Summarize the current simulation mission. List the key vessels and entities involved, their types, and current configuration. Use 'list_entities' to gather information.",
          },
        },
      ],
    };
  }

  throw new Error(`Prompt not found: ${name}`);
});

// Handle tool calls
server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;

  try {
    switch (name) {
      case 'discover_schema': {
        const result = await discoverSchema();
        return {
          content: [
            {
              type: 'text',
              text: JSON.stringify(result, null, 2),
            },
          ],
        };
      }

      case 'list_entities': {
        const result = await listEntities();
        return {
          content: [
            {
              type: 'text',
              text: JSON.stringify(result, null, 2),
            },
          ],
        };
      }

      case 'query_entity': {
        const { id } = args;
        const result = await queryEntity(id);
        return {
          content: [
            {
              type: 'text',
              text: JSON.stringify(result, null, 2),
            },
          ],
        };
      }

      case 'capture_screenshot': {
        const { path, return_base64 = false } = args ?? {};
        const pngBytes = await captureScreenshot();
        const base64 = pngBytes.toString('base64');

        if (path) {
          const abs = resolve(process.cwd(), path);
          await mkdir(dirname(abs), { recursive: true });
          await writeFile(abs, pngBytes);
          return {
            content: [
              {
                type: 'text',
                text: `Saved ${pngBytes.length} bytes to ${abs}`,
              },
            ],
          };
        }

        const content = [
          {
            type: 'image',
            data: base64,
            mimeType: 'image/png',
          },
        ];
        if (return_base64) {
          content.push({ type: 'text', text: base64 });
        }
        return { content };
      }

      case 'execute_command': {
        const { command, params = {} } = args;
        const result = await executeCommand(command, params);

        if (result.error) {
          return {
            content: [
              {
                type: 'text',
                text: `Error: ${result.error}`,
              },
            ],
            isError: true,
          };
        }

        return {
          content: [
            {
              type: 'text',
              text: JSON.stringify(result, null, 2),
            },
          ],
        };
      }

      // ── Model source listing (spec 032) ────────────────────────────
      case 'list_bundled':
      case 'list_open_documents':
      case 'list_twin':
      case 'msl_status':
      case 'list_msl': {
        // Each of these is backed by an ApiQueryProvider on the Rust
        // side. Rather than duplicate the schema/PascalCase mapping for
        // every tool, route them all through the generic executor —
        // their JSON shapes are documented in the static tool entries
        // above.
        const commandMap = {
          list_bundled: 'ListBundled',
          list_open_documents: 'ListOpenDocuments',
          list_twin: 'ListTwin',
          msl_status: 'MslStatus',
          list_msl: 'ListMsl',
        };
        const result = await executeCommand(commandMap[name], args ?? {});
        if (result.error) {
          return {
            content: [{ type: 'text', text: `Error: ${result.error}` }],
            isError: true,
          };
        }
        return {
          content: [
            { type: 'text', text: JSON.stringify(result, null, 2) },
          ],
        };
      }

      case 'open_uri': {
        const { uri } = args ?? {};
        if (!uri) {
          return {
            content: [{ type: 'text', text: 'Error: `uri` is required' }],
            isError: true,
          };
        }
        // `Open` is a Reflect Event on the Rust side, fire-and-forget.
        // We surface it as its own typed tool so agents do not need to
        // remember the PascalCase name or the params shape.
        const result = await executeCommand('Open', { uri });
        if (result.error) {
          return {
            content: [{ type: 'text', text: `Error: ${result.error}` }],
            isError: true,
          };
        }
        return {
          content: [
            { type: 'text', text: JSON.stringify(result, null, 2) },
          ],
        };
      }

      default: {
        // Dynamic tool or unknown command
        if (DYNAMIC_TOOLS.some((t) => t.name === name)) {
          // It's a dynamic command - convert tool name back to command name
          const commandName = name
            .split('_')
            .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
            .join('');

          const result = await executeCommand(commandName, args);

          if (result.error) {
            return {
              content: [
                {
                  type: 'text',
                  text: `Error: ${result.error}`,
                },
              ],
              isError: true,
            };
          }

          return {
            content: [
              {
                type: 'text',
                text: JSON.stringify(result, null, 2),
              },
            ],
          };
        }

        return {
          content: [
            {
              type: 'text',
              text: `Unknown tool: ${name}`,
            },
          ],
          isError: true,
        };
      }
    }
  } catch (error) {
    return {
      content: [
        {
          type: 'text',
          text: `Error: ${error.message}`,
        },
      ],
      isError: true,
    };
  }
});

// Start the server
async function main() {
  console.error(`[LunCoSim MCP] Server starting...`);
  console.error(`[LunCoSim MCP] API: ${process.env.LUNCO_API_HOST || 'localhost'}:${process.env.LUNCO_API_PORT || '3000'}`);

  // Fetch schema on startup to populate dynamic tools
  await refreshSchema();

  const transport = new StdioServerTransport();
  await server.connect(transport);

  console.error('[LunCoSim MCP] Server ready!');
}

main().catch((error) => {
  console.error('[LunCoSim MCP] Failed to start:', error);
  process.exit(1);
});
