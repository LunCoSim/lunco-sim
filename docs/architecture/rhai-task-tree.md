# Rhai task-tree schema

Status: active

This is the contract between Rhai mission policy and the reusable
`lunco-behavior` kernel. Rhai owns which actions, predicates, event names, and
compositions a mission requests. Rust owns parsing, validation, scheduling, and
the fixed-step tree mechanism. The adapter is
[`crates/lunco-scripting/src/task_tree.rs`](../../crates/lunco-scripting/src/task_tree.rs);
the kernel is [`lunco-behavior`](../../crates/lunco-behavior).

## One explicit node schema

Every node is a map with a required string `kind`. There is no inference from
which fields happen to be present, no default leaf, and no fallback spelling.
The prelude constructors in
[`assets/scripting/prelude/tasks.rhai`](../../assets/scripting/prelude/tasks.rhai)
are the authoring API; handwritten maps are useful for tests and generated
policy only when they obey this same schema.

| `kind` | Required fields | Runtime meaning |
|---|---|---|
| `once` | `act` | Call the action once, then succeed. |
| `step` | `act`, `done` | Call the action while the predicate is false. |
| `act_for` | `act`, `secs` | Call the action while the simulation dwell is active. |
| `act_until_event` | `act`, `event`, `src` | Call the action until the named event arrives from the specified gid or path. |
| `wait` | `secs` | Succeed after the simulation dwell. |
| `wait_until` | `done` | Succeed when the predicate returns true. |
| `wait_for` | `event` | Succeed when any emitter produces the event name. |
| `wait_for_from` | `event`, `src` | Succeed for the event from the specified gid or path. |
| `check` | `check` | Return Success/Failure immediately from the predicate. |
| `seq`, `sel`, `all`, `race`, `reactive_seq`, `reactive_sel` | `items` | Compose child nodes. |
| `repeat`, `retry` | `n`, `body` | Repeat a child by count or retry failures. |
| `forever`, `invert`, `force_ok`, `force_fail` | `body` | Apply a decorator to one child. |

Unknown kinds, missing required fields, wrong field types, and fields belonging
to another kind are compile errors. `__bt` is reserved runtime bookkeeping on
the root map and is not authored policy.

The words in `kind` are schema discriminators, not mission text. Event names
such as `"GO"` and emitted telemetry names remain authored protocol strings;
they are intentionally not replaced by a task enum. The Rust adapter parses a
discriminator once into its private `TaskKind` enum, then dispatches only on
that enum and the maintained `lunco-behavior` node constructors.

## Leaf callback contract

Action and predicate fields are anonymous Rhai closures with one positional
argument: `|me| ...`. `me` is the host entity id. The native task driver binds
the persistent scenario-state map as `this` while invoking the closure, so task
policy can use state without a second callback convention. Named
`Fn("...")` pointers are rejected as task leaves; named helpers may still be
called explicitly from an anonymous closure.

## Authoring rule

Production scripts return a tree from `fn task(me)`. The native kernel owns the
cursor, dwell timestamps, event delivery, and terminal status. Production
scripts do not implement a fixed-tick `on_tick` loop. Authored test scenarios
may use `on_tick` only to sample state and publish a bounded verdict, while
`fn mission(me)` is a separate objective/checkpoint policy and may run beside
the task.

```rhai
fn task(me) {
    seq([
        once(|m| drive(m, 1.0, 0.0)),
        wait_until(|m| arrived(m, [10.0, 0.0, 0.0], 2.0)),
        wait_for("SAMPLE_READY"),
    ])
}
```

Use `RunTimeline` or `RunStoredTimeline` for pure data timelines. They lower
into the same `task(me)` contract; they do not create a second progression
engine. Do not add another map tag, alias, compatibility spelling, or private
cursor mechanism. If a new behavior is needed, first check whether an existing
`lunco-behavior` composite/decorator or a typed leaf already owns it; extend
the shared kernel only when the semantics are genuinely reusable.
