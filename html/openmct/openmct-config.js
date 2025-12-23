// OpenMCT Configuration for LunCoSim Telemetry

// Detect if running locally or remotely
const isLocal = window.location.hostname === 'localhost' ||
    window.location.hostname === '127.0.0.1' ||
    window.location.hostname === '';

// Set API URL based on environment
// Local: HTTP to localhost
// Remote: HTTPS to langrenus.lunco.space (uses same TLS certs as WebSocket connections)
const TELEMETRY_API_URL = isLocal
    ? 'http://localhost:8082/api'
    : 'https://langrenus.lunco.space:8082/api';

console.log(`Environment: ${isLocal ? 'LOCAL' : 'REMOTE'}`);
console.log(`Telemetry API URL: ${TELEMETRY_API_URL}`);


// Wait for DOM to be ready
document.addEventListener('DOMContentLoaded', function () {
    console.log('Initializing OpenMCT for LunCoSim...');

    // Initialize OpenMCT
    openmct.setAssetPath('https://cdn.jsdelivr.net/npm/openmct@3.1.0/dist');
    openmct.install(openmct.plugins.LocalStorage());
    openmct.install(openmct.plugins.MyItems());
    openmct.install(openmct.plugins.Espresso());

    // Disable in-memory search to avoid SharedWorker CORS issues
    // Mock SharedWorker to silently fail instead of throwing
    if (window.SharedWorker) {
        const OriginalSharedWorker = window.SharedWorker;
        window.SharedWorker = function (scriptURL) {
            if (scriptURL.includes('inMemorySearchWorker')) {
                console.log('Mocking SharedWorker for in-memory search (CORS workaround)');
                return {
                    port: {
                        start: () => { },
                        postMessage: () => { },
                        addEventListener: () => { },
                        removeEventListener: () => { }
                    },
                    addEventListener: () => { },
                    removeEventListener: () => { }
                };
            }
            return new OriginalSharedWorker(scriptURL);
        };
    }

    // Install time system plugins
    openmct.install(openmct.plugins.UTCTimeSystem());
    openmct.install(openmct.plugins.LocalTimeSystem());

    // LunCoSim Telemetry Plugin
    function LunCoSimTelemetryPlugin() {
        return function install(openmct) {
            console.log('Installing LunCoSim Telemetry Plugin...');

            // Mock Data Generator
            function getMockEntities() {
                return [
                    {
                        entity_id: "mock-rover-1",
                        entity_name: "Mock Rover Alpha",
                        entity_type: "rover"
                    },
                    {
                        entity_id: "mock-base-1",
                        entity_name: "Lunar Base Beta",
                        entity_type: "base"
                    }
                ];
            }

            function getMockTelemetry(id) {
                const now = Date.now();
                return {
                    timestamp: now,
                    "position.x": Math.sin(now / 10000) * 100,
                    "position.y": 10,
                    "position.z": Math.cos(now / 10000) * 100,
                    "velocity.x": Math.cos(now / 10000) * 5,
                    "velocity.y": 0,
                    "velocity.z": -Math.sin(now / 10000) * 5,
                    "controller_id": 1
                };
            }

            function getMockCommands() {
                return [
                    {
                        name: "SET_MOTOR",
                        arguments: [
                            { name: "value", type: "float" }
                        ]
                    },
                    {
                        name: "STOP",
                        arguments: []
                    }
                ];
            }

            // Command discovery and execution
            var commandDefinitions = null;
            function loadCommandDefinitions() {
                return fetch(`${TELEMETRY_API_URL}/command`)
                    .then(response => response.json())
                    .then(data => {
                        commandDefinitions = data.targets;
                        return commandDefinitions;
                    })
                    .catch(err => {
                        console.warn('Could not load command definitions:', err);
                        return {};
                    });
            }

            function executeCommand(targetId, commandName, args) {
                console.log(`Executing command ${commandName} on ${targetId}`, args);
                return fetch(`${TELEMETRY_API_URL}/command`, {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json'
                    },
                    body: JSON.stringify({
                        target_path: targetId,
                        name: commandName,
                        arguments: args
                    })
                }).then(response => response.json());
            }

            // Object provider
            var objectProvider = {
                get: function (identifier) {
                    if (identifier.key === 'luncosim') {
                        return Promise.resolve({
                            identifier: identifier,
                            name: "LunCoSim Entities",
                            type: "folder",
                            location: "ROOT",
                            composition: []
                        });
                    }

                    // Check for mock entities first
                    if (identifier.key.startsWith('mock-')) {
                        const mockEntities = getMockEntities();
                        const isCommands = identifier.key.endsWith('-commands');
                        const baseId = isCommands ? identifier.key.replace('-commands', '') : identifier.key;
                        const entity = mockEntities.find(e => e.entity_id === baseId);

                        if (entity) {
                            if (isCommands) {
                                return Promise.resolve({
                                    identifier: identifier,
                                    name: "Commands",
                                    type: "luncosim.commands",
                                    location: "luncosim:" + baseId
                                });
                            }
                            return Promise.resolve({
                                identifier: identifier,
                                name: entity.entity_name + " (MOCK)",
                                type: "luncosim.telemetry",
                                telemetry: {
                                    values: [
                                        { key: "utc", source: "timestamp", name: "Timestamp", format: "utc", hints: { domain: 1 } },
                                        { key: "position.x", name: "Position X", unit: "m", format: "float", hints: { range: 1 } },
                                        { key: "position.y", name: "Position Y", unit: "m", format: "float", hints: { range: 1 } },
                                        { key: "position.z", name: "Position Z", unit: "m", format: "float", hints: { range: 1 } },
                                        { key: "velocity.x", name: "Velocity X", unit: "m/s", format: "float", hints: { range: 1 } },
                                        { key: "velocity.y", name: "Velocity Y", unit: "m/s", format: "float", hints: { range: 1 } },
                                        { key: "velocity.z", name: "Velocity Z", unit: "m/s", format: "float", hints: { range: 1 } },
                                        { key: "angular_velocity.x", name: "Angular Velocity X", unit: "rad/s", format: "float", hints: { range: 1 } },
                                        { key: "angular_velocity.y", name: "Angular Velocity Y", unit: "rad/s", format: "float", hints: { range: 1 } },
                                        { key: "angular_velocity.z", name: "Angular Velocity Z", unit: "rad/s", format: "float", hints: { range: 1 } },
                                        { key: "rotation.x", name: "Rotation X", unit: "rad", format: "float", hints: { range: 1 } },
                                        { key: "rotation.y", name: "Rotation Y", unit: "rad", format: "float", hints: { range: 1 } },
                                        { key: "rotation.z", name: "Rotation Z", unit: "rad", format: "float", hints: { range: 1 } },
                                        { key: "mass", name: "Mass", unit: "kg", format: "float", hints: { range: 1 } },
                                        { key: "controller_id", name: "Controller ID", format: "integer", hints: { range: 1 } }
                                    ]
                                },
                                location: "luncosim:luncosim"
                            });
                        }
                    }

                    const isCommands = identifier.key.endsWith('-commands');
                    const baseId = isCommands ? identifier.key.replace('-commands', '') : identifier.key;

                    return fetch(`${TELEMETRY_API_URL}/entities`)
                        .then(response => {
                            if (!response.ok) throw new Error('Network response was not ok');
                            return response.json();
                        })
                        .then(data => {
                            const entity = data.entities.find(e => e.entity_id === baseId);
                            if (entity) {
                                if (isCommands) {
                                    return {
                                        identifier: identifier,
                                        name: "Commands",
                                        type: "luncosim.commands",
                                        entity_name: entity.entity_name,
                                        location: "luncosim:" + baseId
                                    };
                                }
                                return {
                                    identifier: identifier,
                                    name: entity.entity_name + " (" + entity.entity_type + ")",
                                    type: "luncosim.telemetry",
                                    entity_name: entity.entity_name,
                                    telemetry: {
                                        values: [
                                            { key: "utc", source: "timestamp", name: "Timestamp", format: "utc", hints: { domain: 1 } },
                                            { key: "position.x", name: "Position X", unit: "m", format: "float", hints: { range: 1 } },
                                            { key: "position.y", name: "Position Y", unit: "m", format: "float", hints: { range: 1 } },
                                            { key: "position.z", name: "Position Z", unit: "m", format: "float", hints: { range: 1 } },
                                            { key: "velocity.x", name: "Velocity X", unit: "m/s", format: "float", hints: { range: 1 } },
                                            { key: "velocity.y", name: "Velocity Y", unit: "m/s", format: "float", hints: { range: 1 } },
                                            { key: "velocity.z", name: "Velocity Z", unit: "m/s", format: "float", hints: { range: 1 } },
                                            { key: "angular_velocity.x", name: "Angular Velocity X", unit: "rad/s", format: "float", hints: { range: 1 } },
                                            { key: "angular_velocity.y", name: "Angular Velocity Y", unit: "rad/s", format: "float", hints: { range: 1 } },
                                            { key: "angular_velocity.z", name: "Angular Velocity Z", unit: "rad/s", format: "float", hints: { range: 1 } },
                                            { key: "rotation.x", name: "Rotation X", unit: "rad", format: "float", hints: { range: 1 } },
                                            { key: "rotation.y", name: "Rotation Y", unit: "rad", format: "float", hints: { range: 1 } },
                                            { key: "rotation.z", name: "Rotation Z", unit: "rad", format: "float", hints: { range: 1 } },
                                            { key: "mass", name: "Mass", unit: "kg", format: "float", hints: { range: 1 } },
                                            { key: "controller_id", name: "Controller ID", format: "integer", hints: { range: 1 } }
                                        ]
                                    },
                                    location: "luncosim:luncosim"
                                };
                            }
                            throw new Error(`Entity not found: ${identifier.key}`);
                        })
                        .catch(err => {
                            console.warn('Error fetching entity:', identifier.key, err);
                            return Promise.reject(err);
                        });
                }
            };

            // Composition provider
            var compositionProvider = {
                appliesTo: function (domainObject) {
                    return domainObject && domainObject.identifier && domainObject.identifier.namespace === 'luncosim' &&
                        (domainObject.identifier.key === 'luncosim' || domainObject.type === 'luncosim.telemetry');
                },
                load: function (domainObject) {
                    if (domainObject.type === 'luncosim.telemetry') {
                        // Commands child for entities
                        return Promise.resolve([{
                            namespace: 'luncosim',
                            key: domainObject.identifier.key + '-commands'
                        }]);
                    }

                    // Root listing
                    return fetch(`${TELEMETRY_API_URL}/entities`)
                        .then(response => {
                            if (!response.ok) {
                                throw new Error('Network response was not ok');
                            }
                            return response.json();
                        })
                        .then(data => {
                            console.log('Loaded entities:', data.entities);
                            let entities = data.entities.map(entity => ({
                                namespace: 'luncosim',
                                key: entity.entity_id
                            }));

                            // Add mock entities if list is empty (for testing)
                            if (entities.length === 0) {
                                console.log('No real entities found, adding mocks...');
                                entities = getMockEntities().map(e => ({
                                    namespace: 'luncosim',
                                    key: e.entity_id
                                }));
                            }
                            return entities;
                        })
                        .catch(err => {
                            console.warn('Could not load entities (server might be offline), using mocks:', err);
                            // Return mock entities on error
                            return getMockEntities().map(e => ({
                                namespace: 'luncosim',
                                key: e.entity_id
                            }));
                        });
                }
            };

            // Telemetry provider
            var telemetryProvider = {
                supportsRequest: function (domainObject) {
                    return domainObject.type === 'luncosim.telemetry';
                },
                request: function (domainObject, options) {
                    if (domainObject.identifier.key.startsWith('mock-')) {
                        return Promise.resolve([getMockTelemetry(domainObject.identifier.key)]);
                    }

                    var url = `${TELEMETRY_API_URL}/telemetry/${domainObject.identifier.key}/history`;
                    var params = new URLSearchParams();

                    if (options.start) {
                        params.append('start', options.start);
                    }
                    if (options.end) {
                        params.append('end', options.end);
                    }

                    return fetch(`${url}?${params}`)
                        .then(response => response.json())
                        .then(data => {
                            console.log('Historical data for', domainObject.identifier.key, ':', data.history?.length || 0, 'samples');
                            return data.history || [];
                        })
                        .catch(err => {
                            console.error('Error fetching history:', err);
                            return [];
                        });
                },
                supportsSubscribe: function (domainObject) {
                    return domainObject.type === 'luncosim.telemetry';
                },
                subscribe: function (domainObject, callback) {
                    var entityId = domainObject.identifier.key;
                    console.log('Subscribing to telemetry for:', entityId);

                    var interval = setInterval(function () {
                        if (entityId.startsWith('mock-')) {
                            callback(getMockTelemetry(entityId));
                            return;
                        }

                        fetch(`${TELEMETRY_API_URL}/telemetry/${entityId}`)
                            .then(response => response.json())
                            .then(data => {
                                if (data && data.timestamp) {
                                    console.log('Telemetry data received:', entityId, 'timestamp:', data.timestamp);
                                    callback(data);
                                } else {
                                    console.warn('Invalid telemetry data (no timestamp):', data);
                                }
                            })
                            .catch(err => console.error('Telemetry fetch error:', err));
                    }, 1000); // Poll every second

                    return function unsubscribe() {
                        console.log('Unsubscribing from:', entityId);
                        clearInterval(interval);
                    };
                },
                supportsMetadata: function (domainObject) {
                    return domainObject.type === 'luncosim.telemetry';
                },
                getMetadata: function (domainObject) {
                    // Return properly structured metadata for OpenMCT
                    if (domainObject.telemetry && domainObject.telemetry.values) {
                        return {
                            values: domainObject.telemetry.values,
                            valueMetadatas: domainObject.telemetry.values
                        };
                    }
                    return domainObject.telemetry;
                }
            };

            // Register providers
            openmct.objects.addRoot({
                namespace: 'luncosim',
                key: 'luncosim'
            });

            openmct.objects.addProvider('luncosim', objectProvider);
            openmct.composition.addProvider(compositionProvider);
            openmct.telemetry.addProvider(telemetryProvider);

            // Command View Provider
            openmct.objectViews.addProvider({
                key: 'luncosim.command-view',
                name: 'Commands',
                cssClass: 'icon-command',
                canView: function (domainObject) {
                    return domainObject.type === 'luncosim.commands';
                },
                view: function (domainObject) {
                    let container;
                    return {
                        show: function (element) {
                            container = element;
                            container.style.padding = '20px';
                            container.style.color = 'white';
                            container.style.overflowY = 'auto';
                            container.style.height = '100%';

                            const title = document.createElement('h2');
                            title.innerText = `Command Control`;
                            container.appendChild(title);

                            const commandList = document.createElement('div');
                            commandList.innerHTML = '<i>Loading commands...</i>';
                            container.appendChild(commandList);

                            const refreshBtn = document.createElement('button');
                            refreshBtn.innerText = 'Refresh Commands';
                            refreshBtn.style.marginBottom = '10px';
                            refreshBtn.style.padding = '5px 10px';
                            refreshBtn.style.cursor = 'pointer';
                            refreshBtn.onclick = renderCommands;
                            container.insertBefore(refreshBtn, commandList);

                            function renderCommands() {
                                commandList.innerHTML = '<i>Loading...</i>';

                                const entityId = domainObject.identifier.key.replace('-commands', '');
                                const entityName = domainObject.entity_name || entityId;

                                const fetchPromise = entityId.startsWith('mock-')
                                    ? Promise.resolve({ [entityId]: getMockCommands() })
                                    : loadCommandDefinitions();

                                fetchPromise.then(targets => {
                                    commandList.innerHTML = '';
                                    // Try lookup by entity name (real) or entityId (mock)
                                    const commands = targets[entityName] || targets[entityId] || [];

                                    if (commands.length === 0) {
                                        commandList.innerHTML = `<p>No commands available for target: ${entityName}.</p>`;
                                        return;
                                    }

                                    commands.forEach(cmd => {
                                        const cmdEl = document.createElement('div');
                                        cmdEl.style.border = '1px solid #555';
                                        cmdEl.style.padding = '10px';
                                        cmdEl.style.marginBottom = '10px';
                                        cmdEl.style.borderRadius = '4px';
                                        cmdEl.style.background = 'rgba(255,255,255,0.05)';

                                        const cmdTitle = document.createElement('h3');
                                        cmdTitle.innerText = cmd.name;
                                        cmdTitle.style.marginTop = '0';
                                        cmdEl.appendChild(cmdTitle);

                                        const argsContainer = document.createElement('div');
                                        argsContainer.style.marginBottom = '10px';

                                        const argInputs = {};
                                        if (cmd.arguments && cmd.arguments.length > 0) {
                                            cmd.arguments.forEach(arg => {
                                                const argRow = document.createElement('div');
                                                argRow.style.marginBottom = '5px';

                                                const label = document.createElement('label');
                                                label.innerText = `${arg.name} (${arg.type}): `;
                                                label.style.width = '150px';
                                                label.style.display = 'inline-block';
                                                argRow.appendChild(label);

                                                const input = document.createElement('input');
                                                input.type = arg.type === 'float' || arg.type === 'int' ? 'number' : 'text';
                                                if (arg.type === 'float') input.step = '0.1';
                                                input.style.background = '#333';
                                                input.style.color = 'white';
                                                input.style.border = '1px solid #666';
                                                input.style.padding = '2px 5px';
                                                argRow.appendChild(input);

                                                argInputs[arg.name] = { input, type: arg.type };
                                                argsContainer.appendChild(argRow);
                                            });
                                        } else {
                                            argsContainer.innerHTML = '<i>No arguments</i>';
                                        }
                                        cmdEl.appendChild(argsContainer);

                                        const execBtn = document.createElement('button');
                                        execBtn.innerText = 'Execute';
                                        execBtn.style.padding = '5px 20px';
                                        execBtn.style.background = '#2196F3';
                                        execBtn.style.color = 'white';
                                        execBtn.style.border = 'none';
                                        execBtn.style.borderRadius = '2px';
                                        execBtn.style.cursor = 'pointer';

                                        const statusMsg = document.createElement('span');
                                        statusMsg.style.marginLeft = '10px';
                                        statusMsg.style.fontSize = '0.9em';

                                        execBtn.onclick = () => {
                                            const args = {};
                                            for (const name in argInputs) {
                                                let val = argInputs[name].input.value;
                                                if (argInputs[name].type === 'float') val = parseFloat(val);
                                                else if (argInputs[name].type === 'int') val = parseInt(val);
                                                args[name] = val;
                                            }

                                            statusMsg.innerText = 'Executing...';
                                            statusMsg.style.color = '#aaa';

                                            if (entityId.startsWith('mock-')) {
                                                setTimeout(() => {
                                                    console.log('Mock command execution:', cmd.name, args);
                                                    statusMsg.innerText = 'Executed (MOCK)';
                                                    statusMsg.style.color = '#4CAF50';
                                                }, 500);
                                                return;
                                            }

                                            executeCommand(entityName, cmd.name, args)
                                                .then(result => {
                                                    if (result.status === 'executed') {
                                                        statusMsg.innerText = 'Success';
                                                        statusMsg.style.color = '#4CAF50';
                                                    } else {
                                                        statusMsg.innerText = 'Error: ' + (result.error || 'Unknown');
                                                        statusMsg.style.color = '#F44336';
                                                    }
                                                })
                                                .catch(err => {
                                                    statusMsg.innerText = 'Failed: ' + err.message;
                                                    statusMsg.style.color = '#F44336';
                                                });
                                        };

                                        cmdEl.appendChild(execBtn);
                                        cmdEl.appendChild(statusMsg);
                                        commandList.appendChild(cmdEl);
                                    });
                                });
                            }

                            renderCommands();
                        },
                        destroy: function () {
                            container = undefined;
                        }
                    };
                }
            });

            // Register types
            openmct.types.addType('luncosim.telemetry', {
                name: 'LunCoSim Telemetry',
                description: 'Telemetry from LunCoSim entities',
                cssClass: 'icon-telemetry'
            });

            openmct.types.addType('luncosim.commands', {
                name: 'Commands',
                description: 'Control interface for the entity',
                cssClass: 'icon-command'
            });

            console.log('LunCoSim Telemetry Plugin installed successfully');
        };
    }

    // Install the plugin
    openmct.install(LunCoSimTelemetryPlugin());

    // Start OpenMCT first
    console.log('Starting OpenMCT...');
    try {
        openmct.start(document.body);
        console.log('OpenMCT started successfully');

        // Set up time system after start
        openmct.time.setClock('local');
        openmct.time.setClockOffsets({ start: -15 * 60 * 1000, end: 0 });
        openmct.time.setTimeSystem('utc');
    } catch (error) {
        console.error('Error starting OpenMCT:', error);
        document.body.innerHTML = '<div style="color: white; padding: 20px;">Error starting OpenMCT: ' + error.message + '</div>';
    }
});
