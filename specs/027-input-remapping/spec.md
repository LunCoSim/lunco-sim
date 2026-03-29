# Feature Specification: 027-input-remapping

**Feature Branch**: `027-input-remapping`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Requirements for flexible, runtime-configurable controller and keyboard mapping.

## Problem Statement
Fixed key bindings in code prevent accessibility and limit the use of professional hardware (Joysticks, HOTAS, Gamepads). We need a centralized input remapping system that allows users to bind physical inputs to semantic simulation actions (e.g., "Throttle", "Steer", "Pan Camera").

## User Stories

### User Story 1 - Semantic Action Mapping
As a user, I want to define my own keyboard and gamepad shortcuts for simulation actions, so that I can tailor the experience to my preferred hardware.

**Acceptance Criteria:**
- The engine uses an input abstraction layer (e.g., `leafwing-input-manager`).
- Users can map multiple physical inputs (e.g., `Space` + `Gamepad South`) to a single semantic action (e.g., `Jump`).

### User Story 2 - Runtime Remapping UI
As a user, I want to change my keybindings through the Unified Editor's UI, so that I don't have to restart the simulation to fix a control conflict.

**Acceptance Criteria:**
- The Unified Editor (`Spec 007`) provides an "Input Settings" window.
- Users can "Listen" for a new key/button press to assign it to an action.
- Bindings are saved to a local config file (e.g., `.ron` or `.json`) and persist between sessions.

### User Story 3 - Controller Hot-Swapping
As an operator, I want to plug in a joystick while the simulation is running and have it immediately recognized.

**Acceptance Criteria:**
- The engine dynamically detects newly connected HID (Human Interface Device) peripherals.
- Default profiles are applied for common controllers (Xbox, DualShock, generic Joysticks).
