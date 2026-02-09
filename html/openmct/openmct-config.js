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
    openmct.install(openmct.plugins.ImageryPlugin.default());
    openmct.install(openmct.plugins.Clock());

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
                    "utc": now,
                    "timestamp": now,
                    "position.x": Math.sin(now / 5000) * 50,
                    "position.y": Math.cos(now / 5000) * 50,
                    "position.z": Math.sin(now / 10000) * 20,
                    "velocity.x": Math.cos(now / 10000) * 5,
                    "velocity.y": 0,
                    "velocity.z": -Math.sin(now / 10000) * 5,
                    "controller_id": 1,
                    "image_url": "https://raw.githubusercontent.com/nasa/openmct/master/example/imagery/imagery/images/img2.jpg"
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
                            name: "LunCoSim",
                            type: "folder",
                            location: "ROOT"
                        });
                    }

                    if (identifier.key === 'entities') {
                        return Promise.resolve({
                            identifier: identifier,
                            name: "LunCoSim Entities",
                            type: "folder",
                            location: "luncosim:luncosim"
                        });
                    }

                    if (identifier.key === 'controllers') {
                        return Promise.resolve({
                            identifier: identifier,
                            name: "Global Controllers",
                            type: "folder",
                            location: "luncosim:luncosim"
                        });
                    }

                    if (identifier.key === 'gallery') {
                        return Promise.resolve({
                            identifier: identifier,
                            name: "Image Gallery",
                            type: "luncosim.gallery",
                            location: "luncosim:luncosim"
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
                                        { key: "controller_id", name: "Controller ID", format: "integer", hints: { range: 1 } },
                                        { key: "image", source: "image_url", name: "Camera Image", format: "image", hints: { image: 1 } }
                                    ]
                                },
                                location: "luncosim:luncosim"
                            });
                        }
                    }

                    // Check for global controllers
                    if (identifier.key.startsWith('global-controller-')) {
                        const targetName = identifier.key.replace('global-controller-', '');
                        return Promise.resolve({
                            identifier: identifier,
                            name: targetName,
                            type: "luncosim.commands",
                            entity_name: targetName,
                            location: "luncosim:controllers"
                        });
                    }

                    const isCommands = identifier.key.endsWith('-commands');
                    const isTelePoint = identifier.key.includes('.');
                    const baseId = isCommands ? identifier.key.replace('-commands', '') :
                        (isTelePoint ? identifier.key.split('.')[0] : identifier.key);
                    const pointKey = isTelePoint ? identifier.key.split('.').slice(1).join('.') : null;

                    return Promise.all([
                        fetch(`${TELEMETRY_API_URL}/entities`).then(r => r.json()),
                        fetch(`${TELEMETRY_API_URL}/dictionary`).then(r => r.json())
                    ]).then(([entityData, dictData]) => {
                        const entity = entityData.entities.find(e => e.entity_id === baseId);
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

                            // Find measurement in dictionary
                            const measurement = dictData.measurements.find(m => m.key === baseId);

                            // Normalize metadata: OpenMCT UTC system expects key "utc"
                            const telemetryValues = (measurement ? measurement.values : []).map(val => {
                                if (val.hints && val.hints.domain && (val.key === 'timestamp' || val.source === 'timestamp')) {
                                    return { ...val, key: 'utc', source: 'timestamp', name: 'Timestamp' };
                                }
                                return val;
                            });

                            // Add a default UTC domain if none found
                            if (!telemetryValues.find(v => v.hints && v.hints.domain)) {
                                telemetryValues.unshift({ key: "utc", source: "utc", name: "Timestamp", format: "utc", hints: { domain: 1 } });
                            }

                            if (isTelePoint) {
                                const valMetadata = telemetryValues.find(v => v.key === pointKey);
                                return {
                                    identifier: identifier,
                                    name: valMetadata ? valMetadata.name : pointKey,
                                    type: "luncosim.telemetry-point",
                                    telemetry: {
                                        values: telemetryValues.filter(v => v.key === 'utc' || v.key === pointKey)
                                    },
                                    location: "luncosim:" + baseId
                                };
                            }

                            return {
                                identifier: identifier,
                                name: entity.entity_name + " (" + entity.entity_type + ")",
                                type: "luncosim.telemetry",
                                entity_name: entity.entity_name,
                                telemetry: {
                                    values: telemetryValues
                                },
                                location: "luncosim:luncosim"
                            };
                        }
                        throw new Error(`Entity not found: ${identifier.key}`);
                    })
                        .catch(err => {
                            console.warn('Error fetching object metadata:', identifier.key, err);
                            return Promise.reject(err);
                        });
                }
            };

            // Composition provider
            var compositionProvider = {
                appliesTo: function (domainObject) {
                    return domainObject && domainObject.identifier && domainObject.identifier.namespace === 'luncosim' &&
                        (domainObject.identifier.key === 'luncosim' ||
                            domainObject.identifier.key === 'entities' ||
                            domainObject.identifier.key === 'controllers' ||
                            domainObject.identifier.key === 'gallery' ||
                            domainObject.type === 'luncosim.telemetry');
                },
                load: function (domainObject) {
                    if (domainObject.type === 'luncosim.telemetry') {
                        // Commands child for entities
                        return Promise.resolve([{
                            namespace: 'luncosim',
                            key: domainObject.identifier.key + '-commands'
                        }]);
                    }

                    if (domainObject.identifier.key === 'luncosim') {
                        // Root listing: Entities and Controllers folders
                        return Promise.resolve([
                            { namespace: 'luncosim', key: 'entities' },
                            { namespace: 'luncosim', key: 'controllers' },
                            { namespace: 'luncosim', key: 'gallery' }
                        ]);
                    }

                    if (domainObject.identifier.key === 'controllers') {
                        // Load all command targets and filter out entities
                        return Promise.all([
                            loadCommandDefinitions(),
                            fetch(`${TELEMETRY_API_URL}/entities`).then(r => r.json()).catch(() => ({ entities: [] }))
                        ]).then(([commandTargets, entityData]) => {
                            const entityNames = new Set(entityData.entities.map(e => e.entity_name));
                            const controllers = [];

                            for (const targetName in commandTargets) {
                                if (!entityNames.has(targetName)) {
                                    controllers.push({
                                        namespace: 'luncosim',
                                        key: 'global-controller-' + targetName
                                    });
                                }
                            }
                            return controllers;
                        });
                    }

                    if (domainObject.identifier.key === 'entities') {
                        // Entities folder listing
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

                    return Promise.resolve([]);
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
                            const history = data.history || [];
                            // Normalize data: ensure 'utc' exists if 'timestamp' is present
                            history.forEach(sample => {
                                if (!sample.utc && sample.timestamp) sample.utc = sample.timestamp;
                            });
                            console.log('Historical data for', domainObject.identifier.key, ':', history.length, 'samples');
                            return history;
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
                                if (data) {
                                    if (!data.utc && data.timestamp) data.utc = data.timestamp;
                                    callback(data);
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
                                        cmdEl.style.padding = '15px';
                                        cmdEl.style.marginBottom = '15px';
                                        cmdEl.style.borderRadius = '6px';
                                        cmdEl.style.background = 'rgba(255,255,255,0.05)';

                                        const cmdHeader = document.createElement('div');
                                        cmdHeader.style.display = 'flex';
                                        cmdHeader.style.justifyContent = 'space-between';
                                        cmdHeader.style.alignItems = 'baseline';
                                        cmdHeader.style.marginBottom = '10px';

                                        const cmdTitle = document.createElement('h3');
                                        cmdTitle.innerText = cmd.name;
                                        cmdTitle.style.margin = '0';
                                        cmdHeader.appendChild(cmdTitle);

                                        if (cmd.description) {
                                            const cmdDesc = document.createElement('div');
                                            cmdDesc.innerText = cmd.description;
                                            cmdDesc.style.fontSize = '0.85em';
                                            cmdDesc.style.color = '#aaa';
                                            cmdDesc.style.fontStyle = 'italic';
                                            cmdEl.appendChild(cmdDesc);
                                            cmdDesc.style.marginBottom = '10px';
                                        }

                                        cmdEl.appendChild(cmdHeader);

                                        const argsContainer = document.createElement('div');
                                        argsContainer.style.marginBottom = '15px';

                                        const argInputs = {};
                                        if (cmd.arguments && cmd.arguments.length > 0) {
                                            cmd.arguments.forEach(arg => {
                                                const argRow = document.createElement('div');
                                                argRow.style.marginBottom = '8px';

                                                const label = document.createElement('label');
                                                label.innerText = `${arg.name}: `;
                                                label.style.width = '120px';
                                                label.style.display = 'inline-block';
                                                argRow.appendChild(label);

                                                let input;
                                                const argType = arg.type || 'string';
                                                const isEnum = argType === 'enum' || argType === 'options';

                                                if (isEnum && arg.values) {
                                                    input = document.createElement('select');
                                                    arg.values.forEach(val => {
                                                        const opt = document.createElement('option');
                                                        opt.value = val;
                                                        opt.text = val;
                                                        input.appendChild(opt);
                                                    });
                                                    if (arg.default) input.value = arg.default;
                                                } else if (argType === 'bool' || argType === 'boolean') {
                                                    input = document.createElement('input');
                                                    input.type = 'checkbox';
                                                    input.checked = arg.default === true || arg.default === 'true';
                                                    input.style.width = 'auto';
                                                } else {
                                                    input = document.createElement('input');
                                                    input.type = argType === 'float' || argType === 'int' || argType === 'number' ? 'number' : 'text';
                                                    if (argType === 'float') input.step = '0.1';
                                                    if (argType === 'vector3') input.placeholder = '[x, y, z]';
                                                    if (arg.default !== undefined) input.value = arg.default;
                                                }

                                                input.style.background = '#333';
                                                input.style.color = 'white';
                                                input.style.border = '1px solid #666';
                                                input.style.padding = '4px 8px';
                                                input.style.borderRadius = '3px';
                                                if (input.type !== 'checkbox') input.style.width = '200px';
                                                argRow.appendChild(input);

                                                if (arg.description) {
                                                    const argDesc = document.createElement('span');
                                                    argDesc.innerText = ` ${arg.description}`;
                                                    argDesc.style.fontSize = '0.8em';
                                                    argDesc.style.color = '#888';
                                                    argDesc.style.marginLeft = '10px';
                                                    argRow.appendChild(argDesc);
                                                }

                                                argInputs[arg.name] = { input, type: argType };
                                                argsContainer.appendChild(argRow);
                                            });
                                        } else {
                                            argsContainer.innerHTML = '<i>No arguments required.</i>';
                                        }
                                        cmdEl.appendChild(argsContainer);

                                        const execBtn = document.createElement('button');
                                        execBtn.innerText = 'Run Command';
                                        execBtn.style.padding = '8px 24px';
                                        execBtn.style.background = '#1e88e5';
                                        execBtn.style.color = 'white';
                                        execBtn.style.border = 'none';
                                        execBtn.style.borderRadius = '4px';
                                        execBtn.style.cursor = 'pointer';
                                        execBtn.style.fontWeight = 'bold';

                                        const statusMsg = document.createElement('span');
                                        statusMsg.style.marginLeft = '15px';
                                        statusMsg.style.fontSize = '0.9em';

                                        execBtn.onclick = () => {
                                            const args = {};
                                            for (const name in argInputs) {
                                                const argCtrl = argInputs[name];
                                                const type = argCtrl.type;
                                                let val = argCtrl.input.type === 'checkbox' ? argCtrl.input.checked : argCtrl.input.value;

                                                if (val === "" || val === undefined || val === null) {
                                                    if (argCtrl.input.type !== 'checkbox') continue;
                                                }

                                                if (type === 'float') val = parseFloat(val);
                                                else if (type === 'int') val = parseInt(val);
                                                else if (type === 'vector3' && typeof val === 'string') {
                                                    try {
                                                        const parsed = JSON.parse(val);
                                                        if (Array.isArray(parsed)) val = parsed;
                                                        else throw new Error("Must be an array [x,y,z]");
                                                    } catch (e) {
                                                        statusMsg.innerText = 'Invalid Vector3 format. Use [0,0,0]';
                                                        statusMsg.style.color = '#F44336';
                                                        return;
                                                    }
                                                }
                                                args[name] = val;
                                            }

                                            statusMsg.innerText = 'Executing...';
                                            statusMsg.style.color = '#aaa';

                                            if (entityId.startsWith('mock-')) {
                                                setTimeout(() => {
                                                    console.log('Mock execution:', cmd.name, args);
                                                    statusMsg.innerText = 'Success (MOCK)';
                                                    statusMsg.style.color = '#4CAF50';
                                                }, 500);
                                                return;
                                            }

                                            executeCommand(entityName, cmd.name, args)
                                                .then(result => {
                                                    if (result.status === 'executed') {
                                                        statusMsg.innerText = '✓ ' + (result.result || 'Success');
                                                        statusMsg.style.color = '#4CAF50';
                                                    } else {
                                                        statusMsg.innerText = '✗ ' + (result.error || 'Unknown Error');
                                                        statusMsg.style.color = '#F44336';
                                                    }
                                                })
                                                .catch(err => {
                                                    statusMsg.innerText = '✗ Failed: ' + err.message;
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

            // Register Gallery View
            openmct.objectViews.addProvider({
                key: 'luncosim.gallery.view',
                name: 'Gallery',
                canView: function (domainObject) {
                    return domainObject.type === 'luncosim.gallery';
                },
                view: function (domainObject) {
                    let container;
                    let interval;
                    return {
                        show: function (element) {
                            container = document.createElement('div');
                            container.style.display = 'grid';
                            container.style.gridTemplateColumns = 'repeat(auto-fill, minmax(200px, 1fr))';
                            container.style.gap = '15px';
                            container.style.padding = '15px';
                            container.style.overflowY = 'auto';
                            container.style.height = '100%';
                            container.style.background = '#111';
                            element.appendChild(container);

                            function refresh() {
                                fetch(`${TELEMETRY_API_URL}/images`)
                                    .then(r => r.json())
                                    .then(data => {
                                        container.innerHTML = '';
                                        if (!data.images || data.images.length === 0) {
                                            container.innerHTML = '<p style="color: #888; text-align: center; grid-column: 1/-1; margin-top: 50px;">No images captured yet. Run the TAKE_IMAGE command on a rover!</p>';
                                            return;
                                        }
                                        data.images.forEach(img => {
                                            const card = document.createElement('div');
                                            card.style.background = '#222';
                                            card.style.borderRadius = '8px';
                                            card.style.padding = '10px';
                                            card.style.cursor = 'pointer';
                                            card.style.transition = 'transform 0.2s';
                                            card.onmouseenter = () => card.style.transform = 'scale(1.02)';
                                            card.onmouseleave = () => card.style.transform = 'scale(1)';

                                            const image = document.createElement('img');
                                            image.src = img.url;
                                            image.style.width = '100%';
                                            image.style.aspectRatio = '16/9';
                                            image.style.objectFit = 'cover';
                                            image.style.borderRadius = '4px';
                                            image.style.display = 'block';

                                            const info = document.createElement('div');
                                            info.style.marginTop = '10px';

                                            const date = document.createElement('div');
                                            date.innerText = new Date(img.timestamp).toLocaleString();
                                            date.style.fontSize = '0.8em';
                                            date.style.color = '#eee';

                                            const name = document.createElement('div');
                                            name.innerText = img.name;
                                            name.style.fontSize = '0.7em';
                                            name.style.color = '#666';
                                            name.style.whiteSpace = 'nowrap';
                                            name.style.overflow = 'hidden';
                                            name.style.textOverflow = 'ellipsis';

                                            info.appendChild(date);
                                            info.appendChild(name);
                                            card.appendChild(image);
                                            card.appendChild(info);
                                            container.appendChild(card);

                                            card.onclick = () => window.open(img.url, '_blank');
                                        });
                                    })
                                    .catch(err => {
                                        container.innerHTML = `<p style="color: #f44336; padding: 20px;">Error loading gallery: ${err.message}</p>`;
                                    });
                            }
                            refresh();
                            interval = setInterval(refresh, 5000);
                        },
                        destroy: function () {
                            if (interval) clearInterval(interval);
                        }
                    };
                }
            });

            // Register types
            openmct.types.addType('luncosim.gallery', {
                name: 'Image Gallery',
                description: 'A view of all images captured by rovers',
                cssClass: 'icon-image'
            });

            openmct.types.addType('luncosim.telemetry-point', {
                name: 'Telemetry Point',
                description: 'A single telemetry channel',
                cssClass: 'icon-telemetry'
            });

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

    // Set up time system BEFORE start to avoid synchronization errors
    try {
        openmct.time.clock('local', { start: -15 * 60 * 1000, end: 0 });
        openmct.time.timeSystem('utc');
    } catch (e) {
        console.warn('Initial clock setup failed, will retry after start:', e.message);
    }

    // Start OpenMCT
    console.log('Starting OpenMCT...');
    try {
        openmct.start(document.body);
        console.log('OpenMCT started successfully');
    } catch (error) {
        console.error('Error starting OpenMCT:', error);
        document.body.innerHTML = '<div style="color: white; padding: 20px;">Error starting OpenMCT: ' + error.message + '</div>';
    }
});
