# LunCo Project Scripts

This directory contains categorized scripts for managing the LunCo simulation project.

## Reorganization Strategy

To keep the project root clean, most scripts have been moved into categorized subdirectories. A master entry point script `lunco.sh` is provided in the project root to access these scripts conveniently.

### Usage

Use the master script in the root directory:
```bash
./lunco.sh <command> <subcommand> [args]
```

---

## Script Categories

### 📂 Setup (`scripts/setup/`)
Scripts for initial environment configuration and dependency installation.

- **`setup_godot_bin.sh`**: Downloads and installs a specific Godot Engine version.
- **`setup_godot_templates.sh`**: Downloads and installs Godot export templates for a specific version.
- **`setup_project_addons.sh`**: Installs project-specific Godot addons using `gd-plug`.
- **`setup_wss_ssl.sh`**: Configures WSS (Secure WebSockets) and handles Let's Encrypt certificates.
- **`manage_wss_certs.sh`**: Advanced utility for WSS maintenance (status, renewal, testing).
- **`copy_ssl_certs.sh`**: Copies SSL certificates to the project for secure server operation.

### 📂 Deployment (`scripts/deploy/`)
Scripts for building and deploying applications to production.

- **`deploy_luncosim_web.sh`**: Builds the 3D simulation for web and deploys it to the web server.
- **`deploy_mcc_openmct.sh`**: Deploys the OpenMCT Mission Control Center to the web server.

### 📂 Execution (`scripts/run/`)
Scripts for running simulation servers and persistent processes.

- **`start_luncosim_server.sh`**: Starts the headless LunCoSim simulation server.
- **`start_luncosim_daemon.sh`**: Starts the LunCoSim server in background mode with a PID file.
- **`web_server.py`**: Local HTTPS server for testing Godot web builds with COOP/COEP headers.
- **`remote_console.py`**: Interactive console for remote simulation control via Telemetry API.

### 📂 Development & CI (`scripts/dev/`)
Scripts for testing, verification, and build management.

- **`test_modelica_app.sh`**: Executes tests for the Modelica application.
- **`gen_git_hash.sh`**: Generates a version file based on the current Git hash.
- **`check_telemetry_api.sh`**: Diagnostic tool to verify telemetry API and entity discovery.
- **`displacement_to_normal.py`**: Utility for generating normal maps from displacement maps.

---

## Adding New Scripts
When adding a new script:
1. Place it in the most appropriate category folder.
2. Use a descriptive name with a category prefix (e.g., `setup_...`, `deploy_...`).
3. Add a descriptive header explaining its purpose and usage.
4. Update the `lunco.sh` master script in the root to include the new command.
5. Update this `README.md` file.
