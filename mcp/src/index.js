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
const SERVER_VERSION = '0.5.0';

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
  // ── Live model interaction (spec 033 P1 + P2) ─────────────────────────
  {
    name: 'describe_model',
    description: "Full structural picture of a Modelica class from the AST. Returns `{class_name, class_kind, extends, components, connections, inputs, parameters, outputs}`. `components` are sub-instances (the diagram boxes) with their declared types and modifications; `connections` are the wiring (`connect(a, b)` equations) as `{from, to}` dot-paths. `inputs / parameters / outputs` carry `type`, `unit`, `default`, `min`, `max`, `description`. Available before compile — comes from the AST. For multi-class docs, pass `class` (short or qualified name); default is the drilled-in class or first non-package class.",
    inputSchema: {
      type: 'object',
      properties: {
        doc: { type: 'integer', description: 'Document id from `list_open_documents`.' },
        class: { type: 'string', description: 'Optional target class. Defaults to drilled-in / first non-package class.' },
      },
      required: ['doc'],
    },
  },
  {
    name: 'snapshot_variables',
    description: 'One-shot read of current values from a running (or paused) sim. Returns `{t, parameters, inputs, variables, ...}`. Pass `names: ["valve", "thrust"]` to filter; omit for everything. Returns `compiled: false` with empty maps if the doc has no linked entity yet (compile not run).',
    inputSchema: {
      type: 'object',
      properties: {
        doc: { type: 'integer', description: 'Document id.' },
        names: {
          type: 'array',
          items: { type: 'string' },
          description: 'Optional filter — names to include (matched across parameters/inputs/variables).',
        },
      },
      required: ['doc'],
    },
  },
  {
    name: 'set_input',
    description: "Push a runtime input value into a compiled model's stepper. Takes effect on the next sim step — no recompile, no pause needed. Errors (warn-log) if the input name is not declared on the model. Use `describe_model` first if unsure of names.",
    inputSchema: {
      type: 'object',
      properties: {
        doc: { type: 'integer', description: 'Document id (0 = active).' },
        name: { type: 'string', description: 'Input variable name (e.g. "valve").' },
        value: { type: 'number', description: 'New value.' },
      },
      required: ['doc', 'name', 'value'],
    },
  },
  // ── Multi-class compile + source visibility (spec 033 P0) ─────────────
  {
    name: 'list_compile_candidates',
    description: 'List the non-package classes in an open document — i.e. the choices the GUI class-picker modal would present. Use this before `compile_model` on a multi-class file to know which class to target. Returns `{candidates: [{qualified, short}], ast_parsed}`.',
    inputSchema: {
      type: 'object',
      properties: {
        doc: { type: 'integer', description: 'Document id from `list_open_documents`.' },
      },
      required: ['doc'],
    },
  },
  {
    name: 'compile_status',
    description: "Read the per-document compile state without triggering a compile. Returns `{state: 'idle'|'compiling'|'ok'|'error', drilled_in_class, picker_pending, candidates, ast_parsed, error_message}`. `picker_pending: true` means the GUI would show its class-picker modal — use `compile_model(doc, class=...)` to bypass it.",
    inputSchema: {
      type: 'object',
      properties: {
        doc: { type: 'integer', description: 'Document id from `list_open_documents`.' },
      },
      required: ['doc'],
    },
  },
  {
    name: 'get_document_source',
    description: 'Fetch the in-memory source text of an open document — including Untitled docs that have no filesystem path. Returns `{source, generation, dirty, kind, origin, title}`. Use this to see what was loaded (including unsaved edits) without re-reading from disk.',
    inputSchema: {
      type: 'object',
      properties: {
        doc: { type: 'integer', description: 'Document id from `list_open_documents`.' },
      },
      required: ['doc'],
    },
  },
  {
    name: 'compile_model',
    description: "Compile an open document. Pass `class` to specify which class to compile — required when the document has multiple non-package classes (the GUI's class-picker modal cannot be invoked through the API). Without `class`, behaves like the GUI Compile button: uses the drilled-in pin, falls back to single-detected-class, otherwise sets `compile_status.picker_pending=true` and aborts.",
    inputSchema: {
      type: 'object',
      properties: {
        doc: { type: 'integer', description: 'Document id (0 = active).' },
        class: { type: 'string', description: 'Target class. Short name (e.g. "RocketStage") or fully qualified. Omit to inherit picker / drilled-in behaviour.' },
      },
      required: ['doc'],
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

      // ── Model source listing (spec 032) + compile/source (spec 033 P0) ─
      case 'list_bundled':
      case 'list_open_documents':
      case 'list_twin':
      case 'list_msl':
      case 'list_compile_candidates':
      case 'compile_status':
      case 'get_document_source':
      case 'describe_model':
      case 'snapshot_variables': {
        // Each of these is backed by an ApiQueryProvider on the Rust
        // side. Rather than duplicate the schema/PascalCase mapping for
        // every tool, route them all through the generic executor —
        // their JSON shapes are documented in the static tool entries
        // above.
        const commandMap = {
          list_bundled: 'ListBundled',
          list_open_documents: 'ListOpenDocuments',
          list_twin: 'ListTwin',
          list_msl: 'ListMsl',
          list_compile_candidates: 'ListCompileCandidates',
          compile_status: 'CompileStatus',
          get_document_source: 'GetDocumentSource',
          describe_model: 'DescribeModel',
          snapshot_variables: 'SnapshotVariables',
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

      case 'set_input': {
        const { doc, name: input_name, value } = args ?? {};
        if (doc === undefined || !input_name || value === undefined) {
          return {
            content: [{ type: 'text', text: 'Error: `doc`, `name`, and `value` are required' }],
            isError: true,
          };
        }
        const result = await executeCommand('SetModelInput', {
          doc,
          name: input_name,
          value,
        });
        if (result.error) {
          return {
            content: [{ type: 'text', text: `Error: ${result.error}` }],
            isError: true,
          };
        }
        return {
          content: [{ type: 'text', text: JSON.stringify(result, null, 2) }],
        };
      }

      case 'compile_model': {
        const { doc, class: target_class } = args ?? {};
        if (doc === undefined || doc === null) {
          return {
            content: [{ type: 'text', text: 'Error: `doc` is required' }],
            isError: true,
          };
        }
        // Map snake_case `class` (the friendly tool field) to the
        // Reflect-event field name. `class` is a JS reserved-ish name
        // so quoting it here keeps the destructure clean.
        const params = { doc };
        if (target_class) params.class = target_class;
        const result = await executeCommand('CompileActiveModel', params);
        if (result.error) {
          return {
            content: [{ type: 'text', text: `Error: ${result.error}` }],
            isError: true,
          };
        }
        return {
          content: [{ type: 'text', text: JSON.stringify(result, null, 2) }],
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
