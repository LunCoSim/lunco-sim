#!/bin/bash
# Quick start script for OpenMCT integration

echo "Starting OpenMCT for LunCoSim..."
echo ""
echo "Make sure Godot/LunCoSim is running first!"
echo "The telemetry API should be available at http://localhost:8082"
echo ""
echo "Starting Python HTTP server on port 8080..."
echo "OpenMCT will be available at: http://localhost:8080/openmct/"
echo ""
echo "Press Ctrl+C to stop the server"
echo ""

cd "$(dirname "$0")"
python3 -m http.server 8080
