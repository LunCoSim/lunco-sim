// OpenMCT Configuration for LunCoSim Telemetry

const TELEMETRY_API_URL = 'http://localhost:8082/api';

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
                        const entity = mockEntities.find(e => e.entity_id === identifier.key);
                        if (entity) {
                            return Promise.resolve({
                                identifier: identifier,
                                name: entity.entity_name + " (MOCK)",
                                type: "luncosim.telemetry",
                                telemetry: {
                                    values: [
                                        { key: "timestamp", name: "Timestamp", format: "utc", hints: { domain: 1 } },
                                        { key: "position.x", name: "Position X", unit: "m", format: "float", hints: { range: 1 } },
                                        { key: "position.y", name: "Position Y", unit: "m", format: "float", hints: { range: 1 } },
                                        { key: "position.z", name: "Position Z", unit: "m", format: "float", hints: { range: 1 } },
                                        { key: "velocity.x", name: "Velocity X", unit: "m/s", format: "float", hints: { range: 1 } },
                                        { key: "velocity.y", name: "Velocity Y", unit: "m/s", format: "float", hints: { range: 1 } },
                                        { key: "velocity.z", name: "Velocity Z", unit: "m/s", format: "float", hints: { range: 1 } },
                                        { key: "controller_id", name: "Controller ID", format: "integer", hints: { range: 1 } }
                                    ]
                                },
                                location: "luncosim:luncosim"
                            });
                        }
                    }

                    return fetch(`${TELEMETRY_API_URL}/entities`)
                        .then(response => {
                            if (!response.ok) throw new Error('Network response was not ok');
                            return response.json();
                        })
                        .then(data => {
                            const entity = data.entities.find(e => e.entity_id === identifier.key);
                            if (entity) {
                                return {
                                    identifier: identifier,
                                    name: entity.entity_name + " (" + entity.entity_type + ")",
                                    type: "luncosim.telemetry",
                                    telemetry: {
                                        values: [
                                            { key: "timestamp", name: "Timestamp", format: "utc", hints: { domain: 1 } },
                                            { key: "position.x", name: "Position X", unit: "m", format: "float", hints: { range: 1 } },
                                            { key: "position.y", name: "Position Y", unit: "m", format: "float", hints: { range: 1 } },
                                            { key: "position.z", name: "Position Z", unit: "m", format: "float", hints: { range: 1 } },
                                            { key: "velocity.x", name: "Velocity X", unit: "m/s", format: "float", hints: { range: 1 } },
                                            { key: "velocity.y", name: "Velocity Y", unit: "m/s", format: "float", hints: { range: 1 } },
                                            { key: "velocity.z", name: "Velocity Z", unit: "m/s", format: "float", hints: { range: 1 } },
                                            { key: "controller_id", name: "Controller ID", format: "integer", hints: { range: 1 } }
                                        ]
                                    },
                                    location: "luncosim:luncosim"
                                };
                            }
                            return null;
                        })
                        .catch(err => {
                            console.warn('Error fetching entity:', err);
                            return null;
                        });
                }
            };

            // Composition provider
            var compositionProvider = {
                appliesTo: function (domainObject) {
                    return domainObject.identifier.namespace === 'luncosim' && domainObject.identifier.key === 'luncosim';
                },
                load: function () {
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
                                    callback(data);
                                }
                            })
                            .catch(err => console.error('Telemetry fetch error:', err));
                    }, 1000); // Poll every second

                    return function unsubscribe() {
                        console.log('Unsubscribing from:', entityId);
                        clearInterval(interval);
                    };
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

            // Register telemetry type
            openmct.types.addType('luncosim.telemetry', {
                name: 'LunCoSim Telemetry',
                description: 'Telemetry from LunCoSim entities',
                cssClass: 'icon-telemetry'
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
