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
| `#[Command]` | struct | Adds `#[derive(Event, Reflect, Clone, Debug)] #[reflect(Event)]` |
| `#[Command(default)]` | struct | …plus `Default` derive and `#[reflect(Default)]` (HTTP-API form) |
| `#[on_command(Type)]` | fn | Wraps with `On<T>`, generates `__register_<fn>(app)` (registers BOTH the type and the observer) |
| `register_commands!(fn_a, fn_b)` | invocation | Generates `pub fn register_all_commands(app)` that calls every `__register_*` for the listed observers |

## Canonical pattern

```rust
use lunco_core::{Command, on_command, register_commands};
use bevy::prelude::*;

// 1. Define the command. `default` = HTTP-API-fillable.
#[Command(default)]
pub struct DriveRover {
    pub target: Entity,
    pub forward: f64,
    pub steer: f64,
}

// 2. Define the observer. The macro keeps `trigger: On<X>` as the
//    synthetic first parameter; bodies can use `cmd.field` (auto-bound
//    via `let cmd = trigger.event();`) or the explicit `trigger`.
#[on_command(DriveRover)]
fn on_drive_rover(trigger: On<DriveRover>, mut q: Query<&mut CommandInputs>) {
    let cmd = trigger.event();
    // cmd.forward, cmd.steer available directly
}

// 3. List all observers in this plugin's command set.
register_commands!(on_drive_rover, on_brake_rover);

// 4. In Plugin::build, one call replaces the whole register_type +
//    add_observer cascade.
impl Plugin for MobilityPlugin {
    fn build(&self, app: &mut App) {
        register_all_commands(app);
    }
}
```

## Field types

- Use **typed identifiers** (`DocumentId`, `Entity`, custom enums), not raw `u64` shims. `lunco-doc::DocumentId` derives `Reflect`; new domain id types should too.
- The HTTP wire layer auto-converts JSON `1` to `DocumentId(1)` via reflection — no manual conversion needed.

## Anti-patterns

```rust
// ✗ Hand-rolled equivalent — drifts from canonical form
#[derive(Event, Reflect, Clone, Debug, Default)]
#[reflect(Event, Default)]
pub struct Foo { … }

// ✗ Manual chain registration — easy to forget either half, no greppable list
app.register_type::<Foo>().add_observer(on_foo);

// ✗ u64 doc-id shim to dodge a Reflect requirement
pub struct Foo { pub doc: u64 }   // use DocumentId
```

## See also

- [lunco-core](../lunco-core/) — re-exports the macros
- [AGENTS.md §4.2](../../AGENTS.md) — canonical pattern + when NOT to use `#[Command]`
- [lunco-mobility](../lunco-mobility/), [lunco-modelica](../lunco-modelica/) — every typed command in those crates uses this triad
