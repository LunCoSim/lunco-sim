# 00 — Create your first Twin

> **Pair with:** the **Twin** and **USD / Modelica** lessons in the luncosim
> workbench.

A Twin is the project that owns one engineered system. It is not a mesh, a
running process, or a Modelica file. It is a directory whose `twin.toml`
identifies the project and whose documents describe the system:

```text
my-rover-twin/
├── twin.toml                 # project identity and entry point
├── scene.usda                # composed 3-D system description
├── models/
│   └── Drive.mo              # continuous equations and state
└── scenarios/
    └── drive.rhai            # mission/test policy
```

The formats keep their normal responsibilities:

| Layer | Owns | Does not own |
|---|---|---|
| `twin.toml` | project identity and the active stage path | physical equations |
| USD | parts, transforms, variants, materials, topology, typed ports, and connections | Modelica state integration |
| Modelica | continuous equations, energy/state balance, and solved outputs | scene traversal or mission sequencing |
| Rhai | events, commands, sequencing, and authored test verdicts | continuous dynamics |
| Rust / Avian | generic projection and engine mechanics | domain-specific names or hidden policy |

## 1. Create the Twin

In the desktop workbench choose **File → New Twin…**, then select or create
the target folder. The command creates `twin.toml` and opens the new Twin.
Headless/API callers can dispatch `CreateTwin { path, name, default_scene }`.

The resulting manifest has this shape:

```toml
name = "my-rover-twin"
version = "0.1.0"
description = "A minimal rover Twin"

[usd]
default_scene = "scene.usda"
```

`default_scene` is the important line: a Twin may contain many USD files, but
only the declared stage is opened as the active world. The other documents are
an asset library until the active stage references them.

## 2. Author the system in USD

USD is the composition and topology layer. A minimal program attachment looks
like this:

```usda
#usda 1.0
(
    defaultPrim = "RoverTwin"
    upAxis = "Y"
    metersPerUnit = 1
)

def Xform "RoverTwin" (
    kind = "assembly"
)
{
    def Scope "Drive" (
        prepend apiSchemas = ["LunCoProgramAPI"]
    )
    {
        # A Twin-owned source uses the registered twin:// authority.
        uniform asset info:sourceAsset = @twin://my-rover-twin/models/Drive.mo@
    }
}
```

For a real vehicle, the same stage would reference authored component assets,
apply standard USD physics schemas, select variants, and connect typed ports:

```usda
float inputs:throttle.connect = </RoverTwin/Drive.outputs:throttle>
```

The connection is a USD fact. The composed stage resolves references, payloads,
variants, and the connection graph; the runtime projects that result. A
composed stage by itself does not execute Modelica, Rhai, physics, or rendering.

## 3. Put continuous behaviour in Modelica

`models/Drive.mo` should contain the equations, not a polling loop. This small
example is intentionally simple: it turns a command into a first-order speed
response and exposes the state as an output.

```modelica
within MyRover;

model Drive
  input Real throttle;
  output Real speed;
  parameter Real timeConstant = 1.0;
initial equation
  speed = 0.0;
equation
  der(speed) = (max(-1.0, min(1.0, throttle)) - speed) / timeConstant;
end Drive;
```

The example shows the ownership boundary. Modelica advances `speed`; it does
not find USD prims or decide when a mission is complete. In a physical Twin,
the same boundary can be an acausal Modelica network: electrical, hydraulic,
thermal, and mechanical domains remain ordinary Modelica components connected
through their declared ports. The runtime mechanism is generic; it does not
infer a domain from a prim name.

## 4. Use Rhai for scenario policy

`scenarios/drive.rhai` is where a mission or test observes the system and sends
semantic commands. It should not reimplement the differential equation:

```rhai
fn on_start(me) {
    this.elapsed = 0.0;
    print("Drive Twin started");
}

fn on_event(me, evt) {
    if evt.name == "DriveComplete" {
        print("Drive observation complete");
        this.done = true;
    }
}
```

Production scenarios observe an authoritative port or event and emit an
authored verdict. A timer is not a physical acceptance condition.

## 5. Run and inspect the Twin

Build the production executable once, then open the declared stage on an
explicit free API port:

```bash
target/debug/luncosim --api 4148 --scene /path/to/my-rover-twin/scene.usda
```

The API port is useful for both a UI session and headless inspection:

```bash
curl -s http://127.0.0.1:4148/api/ready | jq
curl -s http://127.0.0.1:4148/api/commands/schema | jq
```

`/api/ready` proves that the world has crossed its authored readiness gates; it
is not a substitute for observing the behaviour. For an authored scenario gate,
use the production test command:

```bash
target/debug/luncosim test \
  --scene /path/to/my-rover-twin/scene.usda \
  --max-ticks 6000
```

Once `/api/ready` is green, the workbench can capture a real frame through the
same typed command surface used by other clients:

```bash
curl -s -X POST http://127.0.0.1:4148/api/commands \
  -H 'content-type: application/json' \
  -d '{
    "type": "ExecuteCommand",
    "command": "CaptureScreenshot",
    "params": {
      "save_to_file": true,
      "path": "twin-first-frame.png",
      "region": []
    }
  }'
```

When finished, send the typed `Exit` command and verify that the process and
port are gone before opening another Twin. This is important because a Twin's
Modelica participants, handles, timelines, and UI state are scoped to its
lifecycle.

## What to remember

1. A Twin names the project and its active stage.
2. USD authors the composed system facts and topology once.
3. Modelica owns continuous physical equations and state.
4. Rhai owns scenario policy and verdicts.
5. Rust projects generic authored contracts; it does not grow a battery,
   hydraulic, or thermal special case.

Continue with [01 — Lander → Rover mission](01-lander-rover-mission.md) for a
complete multi-domain example, then [03 — Cosim](03-cosim.md) for the live
Modelica/physics communication boundary.
