#!/usr/bin/env node
/**
 * LunCoSim MCP Server
 * 
 * Exposes LunCoSim HTTP API commands as MCP tools for AI agent integration.
 * 
 * Usage:
 *   npx @lunco/mcp-server                    # Use default localhost:3000
 *   npx @lunco/mcp-server --port 8080        # Custom port
 *   npx @lunco/mcp-server --host 192.168.1.100 --port 8080
 */

import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from '@modelcontextprotocol/sdk/types.js';

// Configuration
const args = process.argv.slice(2);
let apiHost = 'localhost';
let apiPort = 3000;

for (let i = 0; i < args.length; i++) {
  if (args[i] === '--host' && i + 1 < args.length) {
    apiHost = args[i + 1];
    i++;
  } else if (args[i] === '--port' && i + 1 < args.length) {
    apiPort = parseInt(args[i + 1], 10);
    i++;
  }
}

const API_BASE_URL = `http://${apiHost}:${apiPort}`;

// MCP Server instance
const server = new Server(
  {
    name: 'lunco-mcp-server',
    version: '0.1.0',
  },
  {
    capabilities: {
      tools: {},
    },
  }
);

// Tool definitions
const tools = [
  {
    name: 'execute_command',
    description: 'Execute a LunCoSim command by name with optional parameters. Commands are discovered via schema discovery.',
    inputSchema: {
      type: 'object',
      properties: {
        command: {
          type: 'string',
          description: 'The command name to execute (e.g., "DriveRover", "SpawnVessel")',
        },
        params: {
          type: 'object',
          description: 'Optional parameters for the command as a JSON object',
          default: {},
        },
      },
      required: ['command'],
    },
  },
  {
    name: 'discover_schema',
    description: 'Discover available commands and their parameters in the LunCoSim API.',
    inputSchema: {
      type: 'object',
      properties: {},
    },
  },
  {
    name: 'list_entities',
    description: 'List all entities in the current LunCoSim simulation.',
    inputSchema: {
      type: 'object',
      properties: {},
    },
  },
  {
    name: 'query_entity',
    description: 'Get details about a specific entity by its ID.',
    inputSchema: {
      type: 'object',
      properties: {
        id: {
          type: 'string',
          description: 'The entity ID (ULID format)',
        },
      },
      required: ['id'],
    },
  },
];

// Helper: make API request
async function apiRequest(endpoint, body = null) {
  const url = `${API_BASE_URL}${endpoint}`;
  const options = {
    method: body ? 'POST' : 'GET',
    headers: {
      'Content-Type': 'application/json',
    },
  };
  if (body) {
    options.body = JSON.stringify(body);
  }

  const response = await fetch(url, options);
  const data = await response.json();
  return data;
}

// Handle tool listing
server.setRequestHandler(ListToolsRequestSchema, async () => {
  return { tools };
});

// Handle tool calls
server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;

  try {
    switch (name) {
      case 'execute_command': {
        const { command, params = {} } = args;
        const result = await apiRequest('/api/command', {
          command,
          params,
        });
        
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

      case 'discover_schema': {
        const result = await apiRequest('/api/schema');
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
        const result = await apiRequest('/api/entities');
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
        const result = await apiRequest(`/api/entity/${id}`);
        return {
          content: [
            {
              type: 'text',
              text: JSON.stringify(result, null, 2),
            },
          ],
        };
      }

      default:
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
  console.error(`LunCoSim MCP Server starting...`);
  console.error(`Connecting to LunCoSim API at ${API_BASE_URL}`);
  
  const transport = new StdioServerTransport();
  await server.connect(transport);
  
  console.error('LunCoSim MCP Server ready!');
}

main().catch((error) => {
  console.error('Failed to start MCP server:', error);
  process.exit(1);
});
