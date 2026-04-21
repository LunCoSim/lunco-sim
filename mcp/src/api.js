/**
 * HTTP client for LunCoSim API.
 *
 * All communication goes through POST /api/commands with a unified request envelope.
 * Uses the tagged format: {"type": "ExecuteCommand", "command": "...", "params": {...}}
 */

const API_BASE_URL = `http://${process.env.LUNCO_API_HOST || 'localhost'}:${process.env.LUNCO_API_PORT || '3000'}`;

/**
 * Make a request to the LunCoSim API.
 * @param {Object} request - API request object
 * @returns {Promise<Object>} API response
 */
export async function apiRequest(request) {
  const url = `${API_BASE_URL}/api/commands`;
  const options = {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request),
  };

  const response = await fetch(url, options);
  if (!response.ok) {
    throw new Error(`API request failed: ${response.status} ${response.statusText}`);
  }

  const data = await response.json();
  return data;
}

/**
 * Execute a typed command by name with parameters.
 * @param {string} command - Command name (e.g. "DriveRover")
 * @param {Object} params - Command parameters
 * @returns {Promise<Object>} Execution result
 */
export async function executeCommand(command, params = {}) {
  return apiRequest({
    type: 'ExecuteCommand',
    command,
    params,
  });
}

/**
 * Discover all available commands and their parameter schemas.
 * @returns {Promise<Object>} Schema with commands array
 */
export async function discoverSchema() {
  return apiRequest({ type: 'DiscoverSchema' });
}

/**
 * List all entities in the simulation.
 * @returns {Promise<Object>} Entity list with count
 */
export async function listEntities() {
  return apiRequest({ type: 'ListEntities' });
}

/**
 * Query a specific entity by API ID.
 * @param {string} apiId - The entity's API ID (ULID format)
 * @returns {Promise<Object>} Entity details
 */
export async function queryEntity(apiId) {
  return apiRequest({
    type: 'QueryEntity',
    id: apiId,
  });
}

/**
 * Capture a screenshot and return PNG bytes.
 * @returns {Promise<Buffer>} Raw PNG bytes
 */
export async function captureScreenshot() {
  const response = await fetch(`${API_BASE_URL}/api/commands`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      type: 'ExecuteCommand',
      command: 'CaptureScreenshot',
      params: { save_to_file: false },
    }),
  });

  if (!response.ok) {
    throw new Error(`Screenshot failed: ${response.status}`);
  }

  // PNG bytes returned directly for screenshots
  const pngBytes = await response.arrayBuffer();
  return Buffer.from(pngBytes);
}
