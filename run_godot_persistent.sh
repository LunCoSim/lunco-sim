#!/bin/bash
# Start Godot in the background and keep it running
nohup godot --server > godot_server.log 2>&1 &
echo $! > godot.pid
echo "Godot started with PID $!"
