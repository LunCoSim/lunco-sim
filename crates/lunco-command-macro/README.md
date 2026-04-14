# lunco-command-macro

Proc-macro crate for LunCoSim's typed command system.

**Users never import this crate directly.** All macros are re-exported from `lunco_core`:

```rust
use lunco_core::{Command, on_command, register_commands};
```

## Why a separate crate?

Rust requires `proc_macro` crates to be compiled separately from regular crates. This crate contains only compile-time code (~150 lines) with zero runtime dependencies.

## What it provides

Three macros, all re-exported from `lunco_core`:

| Macro | Target | What it does |
|---|---|---|
| `#[Command]` | struct | Replaces `#[derive(Event, Reflect, Clone, Debug)]` |
| `#[on_command(Type)]` | fn | Wraps with `On<T>`, generates `__register_<fn>(app)` |
| `register_commands!(fn_a, fn_b)` | invocation | Generates `pub fn register_all_commands(app)` |

## Example

```rust
use lunco_core::{Command, on_command, register_commands};
use bevy::prelude::*;

#[Command]
pub struct DriveRover {
    pub target: Entity,
    pub forward: f64,
    pub steer: f64,
}

#[on_command(DriveRover)]
fn on_drive_rover(cmd: DriveRover, mut q: Query<&mut FlightSoftware>) {
    // cmd.forward, cmd.steer available directly
}

register_commands!(on_drive_rover, on_brake_rover);
```

## See also

- [lunco-core](../lunco-core/) — re-exports the macros, defines `SimCommand` trait
- [lunco-mobility](../lunco-mobility/) — example usage (`DriveRover`, `BrakeRover`)
