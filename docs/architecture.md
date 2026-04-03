# LunCoSim View & Intent Architecture

This document provides a technical guide to the modular, action-oriented, and headless-safe camera and intent systems in LunCoSim.

---

## 1. The 5-Layer Control Model
LunCoSim decouples human interaction from physical execution using five distinct layers:

| Layer | Name | Responsibility | Logical Flow |
| :--- | :--- | :--- | :--- |
| **5** | **UserIntent** | **Semantic Mapping**: Raw inputs (WASD, Mouse) -> Abstract Goals (`MoveForward`, `LookAtTarget`). | Keyboard -> `Leafwing` -> `UserIntent` |
| **4** | **Controller** | **Translation**: Translates `UserIntent` into specific `CommandMessages` or `Actions` for a target entity. | `UserIntent` -> `Avatar` -> `CommandMessage` |
| **3** | **FSW / Subsystem**| **The Brain**: Decentralized observers that execute commands and emit `CommandResponse` ACKs. | `CommandMessage` -> `Subsystem Observer` -> `ACK` |
| **2** | **Logic / Device** | **Hardware Logic**: The individual components (e.g., `CameraDevice`, `ViewPoint`, `Motor`) responding to state changes. | `Subsystem` -> `Component Field` |
| **1** | **Plant / Physics**| **Mechanical Truth**: The `f64` spatial state and physical physics interaction. | `Component Field` -> `DVec3` / `Physics Impulse` |

---

## 2. Vision Components: ViewPoint vs. CameraDevice

### **ViewPoint (Logical)**
The universal logical "eye." 
- **Crate**: `lunco-core` (Headless Safe).
- **Purpose**: Defines where an entity is looking and its FOV. Both bots and players read this component to perform spatial math (e.g., "Is the Earth in the center of my ViewPoint?").
- **Precision**: Uses `f64` for planetary-scale accuracy.

### **CameraDevice (Physical)**
Representing a sensing hardware unit.
- **Crate**: `lunco-core` (Hardware Marker).
- **Purpose**: Attaches a `ViewPoint` to a physical presence. It can optionally have a **Physical Collider** (via `avian`) to prevent terrain clipping.

### **Renderer / Blender (Visual)**
The rendering bridge.
- **Crate**: `lunco-camera` (Client Only).
- **Purpose**: Syncs a Bevy `Camera3d` and its `FloatingOrigin` to the active `ViewPoint`. It handles interpolation (smoothing) between the logical truth and the rendered frame.

---

## 3. The Lifecycle: Command -> Action

### **CommandMessage** (The Pulse)
A discrete instruction packet.
- **Dumb Transport**: The envelope is lean (`id`, `target`, `source`, `name`, `args`). It does NOT handle spatial context; the receiving FSW handles internal coordinate mapping.
- **Performance**: Arguments use **`SmallVec<[f64; 4]>`** to stay on the stack for high-frequency ticks (WASD).
- **Feedback**: Every command triggers a **`CommandResponse`** (ACK/NACK) pulse for Mission Control confirmation.

### **ActiveAction** (The Process)
A long-running, stateful task with a lifecycle:
1. **Started**: A `CommandMessage` triggers an `ActiveAction`.
2. **Running**: A dedicated system updates `progress` and modifies the target component.
3. **Preemption**: Manual USER input (via `UserIntent`) immediately cancels active actions to ensure tactile control responsiveness.
4. **Result**: Upon completion, a final `CommandResponse` is emitted.

---

## 4. Input Preemption
To provide a natural "human" feel, manual user input always takes precedence over automated actions:
- If a `Look` or `Move` intent is detected, the `Avatar` automatically **Cancels** any active `CameraTransition` actions.
- Controls are "handed over" to the USER immediately, preventing fights between manual steering and automated transitions.

---

## 5. Headless Compatibility
The simulation core (`lunco-celestial`, `lunco-core`) has NO dependency on `lunco-camera` or Bevy's rendering systems.
- **Bots** can "see" and "look at" objects by reading and writing to `ViewPoint` components through the same `Action` system.
- **Server** instances run the full spatial logic without a GPU.
- **Clients** add the `LunCoCameraPlugin` to provide the visual bridge and post-processing effects.
