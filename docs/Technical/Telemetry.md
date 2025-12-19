# Telemetry & OpenMCT

LunCoSim provides a robust telemetry stream compatible with **NASA OpenMCT** (Open Mission Control Technologies).

## Architecture
-   **TelemetryManager**: A singleton that automatically discovers entities (`VehicleBody3D`, etc.) and records their properties (position, velocity, internal state).
-   **TelemetryServer**: An HTTP/WebSocket server (default port `8082`) that exposes this data.
-   **OpenMCT UI**: A web-based dashboard running on port `8080` (default) or hosted externally.

## Setup
1.  **Deploy OpenMCT**:
    Run the deployment script to copy the web dashboard files to your local web server (requires `nginx` or `apache`).
    ```bash
    ./deploy_openmct.sh
    ```
2.  **Start Simulation**: Open LunCoSim and run a mission. The `TelemetryServer` starts automatically.
3.  **Open Dashboard**: Navigate to `http://localhost/mcc/` (or your configured path).

## Data Flow
1.  **Collection**: `TelemetryManager` polls entities at 2Hz.
2.  **Storage**: Keeps a rolling buffer of history (last 1000 samples).
3.  **Serving**:
    -   `/api/dictionary`: Tells OpenMCT what measurements are available.
    -   `/api/telemetry/{id}`: Provides real-time data.
    -   `/api/history/{id}`: Provides historical data for plots.

## Adding Custom Telemetry
Any variable in your Entity's `Telemetry` dictionary (if implemented) or reflected properties will be automatically picked up by the manager.
