# Feature Specification: 004-simulation-time

**Feature Branch**: `004-simulation-time`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Time decoupling requirements for SIL/HIL and ML training.

## Problem Statement
Bevy's default `Time` resource is bound to the system clock (real time). To serve as a digital twin and testing platform, the simulation must be able to run "Faster than Real-time" for ML/Orbital mechanics, or in "Lockstep" for debugging Fprime/ROS nodes. 

## User Scenarios

### User Story 1 - Fast-Forward Headless Execution (Priority: P1)
As a machine learning engineer, I want the simulation to run as fast as my CPU allows, so that I can train an autonomous navigation model over thousands of lunar days quickly.

**Acceptance Criteria:**
- The engine can be configured to ignore the 60hz Vsync cap and update the physics/sensor loops sequentially.
- The `Time::delta()` passed to systems is a fixed constant rather than wall-clock time.

### User Story 2 - Lockstep Time Sync (Priority: P1)
As a flight software engineer, I want the Bevy simulation to wait for my Fprime FSW to process the last sensor frame before stepping forward, so that testing overhead doesn't ruin my breakpoint debugging.

**Acceptance Criteria:**
- The engine pauses its primary `Update` loop until a "Step" command is received via the Universal Control bridge.
- External software receives the simulated timestamp to ensure log alignment.
