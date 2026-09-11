# Behaviour trees

LunCoSim uses a small, language-neutral tree kernel for reusable task
mechanics. Authored behavior is Rhai policy: a scenario returns a task tree,
and the runtime ticks that tree against its generic context.

- **Kernel:** [`lunco-behavior`](../crates/lunco-behavior) owns `Status`, node
  traversal, reset, ordered composites, parallel/race, loops, reactive
  composites, and decorators. It has no knowledge of vehicles, routes, USD,
  Modelica, Avian, or Rhai.
- **Binding:** [`lunco-scripting`](../crates/lunco-scripting/src/task_tree.rs)
  validates Rhai data once, converts it to the kernel's typed nodes, and gives
  leaves access to the public scripting bridge.
- **Policy:** `.rhai` sources choose subjects, route points, events, tolerances,
  and outcomes. A scene-level `LunCoProgramAPI` prim can own a route program;
  route geometry is composed USD, not a list stored on a vehicle.

## Authored shape

```rhai
fn task(me) {
    reactive_sel([
        seq([
            once(|m| nav_to(m, [10.0, 0.0, -20.0], 0.6, 2.0)),
            wait_until(|m| arrived(m, [10.0, 0.0, -20.0], 2.0)),
        ]),
        once(|m| brake(m)),
    ])
}
```

The prelude constructors are the only script-facing schema:

- Leaves: `once`, `step`, `act_for`, `act_until_event`, `wait`,
  `wait_until`, `wait_for`, `wait_for_from`, and `check`.
- Ordered/concurrent nodes: `seq`, `sel`, `par_all`, `par_race`,
  `reactive_seq`, and `reactive_sel`.
- Decorators: `repeat`, `forever`, `retry`, `invert`, `force_ok`, and
  `force_fail`.

Leaves use anonymous closures. The runtime keeps the cursor and dwell state;
Rhai keeps only authored policy and durable `this` state needed by hooks. A
task action may emit a typed tool event, while the generic tool registry and
Bevy adapter own execution of registered executable tools.

## Runtime contract

`Running` means the node is retained for the next fixed step. `Success` and
`Failure` are terminal for that node; composites reset children when they are
re-entered. `repeat` and `forever` therefore re-enter a fresh child and restart
its dwell clock. Event leaves match the bounded event identity (`name` and
optional source); full payloads remain available to the scenario's `on_event`
hook.

Reactive composites re-evaluate their guards from the first child each tick.
Use them when a higher-priority safety or hold condition must preempt a running
action. Use a plain sequence/selector when a started action should retain its
cursor until it finishes.

The task kernel does not calculate vehicle dynamics. `nav_command`, named
ports, Modelica equations, sensors, and physics remain their owning generic
mechanisms. A route program reacts to sensor enter events and publishes the
current named-port command; physics does not publish a route-specific “target
reached” fact.

## Testing boundary

The production authored contract is
[`assets/scenarios/tests/scripting_task_contract.rhai`](../assets/scenarios/tests/scripting_task_contract.rhai),
loaded by
[`scripting_task_contract.usda`](../assets/scenes/tests/scripting_task_contract.usda).
It covers sequencing, event waits, dwell, parallel/race, repetition,
non-terminating tasks, and gated mission completion through the same production
event/task surface used by authored programs.

Rust keeps only the malformed-shape compiler check in `task_tree.rs`, because
that check has no smaller public runtime observation. Do not add one Rust test
per constructor or copy route/mission policy into a fake context. When a new
observable policy is needed, add it to an authored scene-test scenario and
emit one bounded verdict.

See [the scripting guide](scripting-guide.md) for commands and
[the route architecture](architecture/waypoints-in-usd.md) for USD ownership.
