# Tutorial 01 — Build a Lander → Rover Mission

In this tutorial you'll build a small lunar mission from scratch. A lander flies
itself down to the surface on a glowing engine plume and settles on its legs. It
then releases a rover, which drives itself through a course of glowing waypoints —
until you grab the controls and take over. Along the way the screen narrates each
phase.

You'll write three kinds of files, all under `assets/`:

- **`.usda`** files — the world and the vehicles in it;
- a **`.mo`** Modelica model — the lander's flight control law;
- a few **`.rhai`** scripts — the mission logic and behaviours.

Edit a file, reload the scene, watch it change. That's the whole loop. Let's go.

Everything you create is prefixed `my_` so it sits alongside the shipped assets
rather than on top of them. The finished article is already in the repo — peek at
`assets/vessels/landers/descent_lander.usda` and `assets/scenes/sandbox/lander_test.usda`
whenever you want to see where this is heading, or play it from the 🎓 Tutorials
menu in the sandbox.

You can run your work at any point with:

```
cargo run -p lunco-sandbox --bin sandbox -- --api 4101 --scene scenes/sandbox/my_mission.usda
```

---

## A quick mental model

Before the first file, three distinctions that make everything else click.

A **signal** is a value that always has a current reading — altitude, throttle,
fuel remaining, gravity. You *read* a signal whenever you want it. Models publish
signals; scripts read them with `get(...)`.

An **event** is a thing that happens once — a touchdown, a rover crossing a finish
line, a key press, the fuel dropping below a line. You don't poll for events; you
*react* to them. Scripts fire events with `emit(...)` and listen with `on_event` /
`wait_for`.

**Authority** is the answer to "who is driving this vehicle right now?" A vehicle
is either flying itself or being flown by somebody — a human at the keyboard, or an
autopilot script. Taking the controls is called **possessing** the vehicle, and it
is the one source of truth: nothing in the sim guesses at who's in charge, it asks.

Whenever you're unsure where some logic belongs, ask: is this a value I read, a
moment I react to, or a question about who's driving? Signal, event, authority.
Keep them straight and the rest is easy.

---

## Step 1 — Lay down the world

Start with the stage: a floor to land on, a camera to watch from, and a sun.

Create `assets/scenes/sandbox/my_mission.usda`:

```usda
#usda 1.0
(
    defaultPrim = "Mission"
    upAxis = "Y"
    metersPerUnit = 1
)

def Xform "Mission"
{
    def Cube "Ground" ( prepend apiSchemas = ["PhysicsCollisionAPI"] )
    {
        double size = 1.0
        double3 xformOp:translate = (0, -0.5, 0)
        double3 xformOp:scale = (200.0, 1.0, 200.0)
        uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:scale"]
        color3f primvars:displayColor = (0.35, 0.33, 0.38)
        bool physics:collisionEnabled = true
        float physics:friction = 1.0
    }

    def Xform "Avatar" ( prepend apiSchemas = ["LuncoAvatarAPI"] )
    {
        uniform bool lunco:avatar = true
        uniform token lunco:cameraMode = "freeflight"
        double3 xformOp:translate = (18.0, 12.0, 20.0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }

    def DistantLight "Sun"
    {
        float inputs:intensity = 12000
        double3 xformOp:rotateXYZ = (-30.0, 45.0, 0.0)
        uniform token[] xformOpOrder = ["xformOp:rotateXYZ"]
    }
}
```

A USD scene is a tree of *prims*. The ground is a big flat cube with a collider but
no rigid body, so it stays put and nothing falls through it. `lunco:avatar` marks the
camera the player looks through. Run it now — you'll see an empty grey plain. Not much
yet, but it's solid ground.

A couple of physics rules worth knowing as you go: a prim with a collider and
**no** rigid body is *static* (the ground). Add a rigid body and a mass and it
becomes *dynamic* — it falls and reacts to forces. That's what the lander will be.

---

## Step 2 — Teach the lander how to fly

Our lander shouldn't just drop — it should *fly itself down*, easing onto its legs
like a real one. That flight law is a small controller, and the natural place for
control laws is Modelica.

Create `assets/models/MyLander.mo`:

```modelica
model MyLander
  parameter Real max_thrust = 60000.0;
  parameter Real v_e = 2900.0;            // exhaust velocity (for fuel book-keeping)

  // Inputs — anything you might want to change while it runs (gains, set-point).
  // Keep these as `input`, not `parameter`, so you can retune them live.
  input Real vehicle_mass = 2000.0;
  input Real kv = 1.2;                    // descent-rate tracking gain
  input Real rest_altitude = 1.5;         // altimeter range (m) at leg contact
  input Real descent_slope = 0.6;         // how much faster to fall, per m of height
  input Real vy_max = 6.0;                // never fall faster than this
  input Real g = 1.62;

  // These come from outside: what the vehicle senses about itself.
  input Real altitude = 60.0;
  input Real descent_rate = 0.0;

  // Authority. 1 when somebody is driving, 0 when the vehicle flies itself.
  // You never set this — it is WIRED from the vehicle's possession state (Step 4).
  input Real piloted = 0.0;
  input Real external_throttle = 0.0;     // the pilot's stick, when there is a pilot
  input Real pitch = 0.0;                 // W / S
  input Real roll = 0.0;                  // A / D
  input Real yaw = 0.0;                   // Q / E

  output Real thrust;                     // N, along the lander's OWN +Y
  output Real torque_x;
  output Real torque_y;
  output Real torque_z;
  output Real throttle;                   // 0..1, how hard it's firing

  parameter Real torque_gain = 30000.0;    // N.m per unit of stick deflection

  Real m_prop(start = 2000.0);
  Real vy_sched, target_vy, a_cmd, gnc_raw, gnc_pos, gnc_thrust, gnc_throttle;
  Real cmd_throttle;
equation
  // Descent schedule: fall faster when high, slow to a crawl near the pads.
  vy_sched = min(max(descent_slope * (altitude - rest_altitude), 0.0), vy_max);
  target_vy = -vy_sched;

  // Track that target descent rate, feeding gravity forward.
  a_cmd = g + kv * (target_vy - descent_rate);
  gnc_raw = vehicle_mass * a_cmd;
  gnc_pos = max(gnc_raw, 0.0);
  gnc_thrust = min(gnc_pos, max_thrust);
  gnc_throttle = gnc_thrust / max_thrust;

  // THE AUTHORITY GATE. Blend, don't branch: when `piloted` is 1 the pilot's stick
  // wins; when it's 0 the internal law wins. No flag, no mode variable, no script.
  cmd_throttle = piloted * external_throttle + (1.0 - piloted) * gnc_throttle;

  thrust = cmd_throttle * max_thrust;
  throttle = cmd_throttle;

  // Attitude is the pilot's alone. The same `piloted` gate multiplies every stick
  // axis to zero when nobody is driving, so the autonomous descent issues no torque
  // and comes down upright. No mode flag, no second code path.
  torque_x = piloted * pitch * torque_gain;   // pitch about X
  torque_y = piloted * yaw * torque_gain;     // yaw   about Y
  torque_z = piloted * roll * torque_gain;    // roll  about Z

  der(m_prop) = -thrust / v_e;
end MyLander;
```

Read it top to bottom and it's just a descent controller: work out how fast we
*should* be falling at this height, track that speed, feed gravity forward, clamp to
a sane thrust. The `throttle` output (0..1) is the engine's "how hard am I firing
right now" — we'll use it for the flame later.

The lines to dwell on are `cmd_throttle` and the three torques. That is the authority
gate, and it's the whole reason a human can grab this vehicle mid-descent without
anything fighting them. `piloted` isn't a variable you toggle; in Step 4 you'll
**wire** it to the vehicle's possession state, so it becomes 1 the instant someone
takes the controls. The model doesn't know or care whether that someone is a person
or an autopilot.

Watch what the gate buys you. Throttle *blends* — the pilot's stick fades in as the
GNC fades out. Attitude is multiplied outright, so an unpiloted lander commands zero
torque and rides its descent bolt upright, while a piloted one answers W/A/S/D
immediately. One `piloted` signal, two different behaviours, no `if`.

Where `pitch`, `roll` and `yaw` come from is worth knowing: nothing in this file, and
no script. The `Controls` profile you'll reference in Step 3 maps the keyboard's
intents onto ports *by name* — `action → external_throttle`, `forward → pitch`,
`left → roll`, `yaw_left → yaw`. Name an input differently and that key quietly does
nothing, which is a genuinely annoying afternoon.

One simplification worth flagging: these torques act about the **world** axes, not
the lander's. Upright that's the same thing, and it keeps the model readable. Tip
past about 45° and "pitch" starts meaning something other than what your hands
expect. The shipped `Lander.mo` does it properly — it takes the body quaternion as
an input and rotates body-frame torque into world before emitting it.

Three things will trip you up if nobody warns you, so here they are:

- **Write clamps and mode-switches as flat `if`s, `min`/`max`, or arithmetic —
  never nested `if/else if`.** The gate above is a multiply-and-add for exactly this
  reason. Chained `else if` doesn't translate reliably yet.
- **Don't make decisions inside the model based on a wired-in input.** A line like
  `when altitude < 0.2` won't see the live altitude. When you need a threshold like
  that, detect it *outside* the model — Step 6 shows how.
- **Never hard-code gravity.** `g` is an input for a reason; wire it, or your lander
  misbehaves the moment it's somewhere else.

---

## Step 3 — Make the lander a vehicle of its own

Here's a decision worth making early: **a vehicle is not part of a scene.** The
lander has a body, an engine, sensors, a control law and a wiring loom, and none of
that changes when you fly it somewhere else. Only where it *starts* changes.

So the lander gets its own file, and the scene will reference it — exactly like the
rover you'll pull in at Step 7. Create `assets/vessels/landers/my_lander.usda`:

```usda
#usda 1.0
(
    defaultPrim = "MyLander"
    upAxis = "Y"
    metersPerUnit = 1
)

def Cylinder "MyLander" ( prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI", "PhysicsMassAPI", "LuncoVesselAPI", "LuncoProgramAPI"] )
{
    uniform token axis = "Y"
    double radius = 2.5
    double height = 3.0

    bool physics:rigidBodyEnabled = true
    bool physics:collisionEnabled = true
    float physics:mass = 2000.0

    # Gold multi-layer-insulation foil. Just a colour — see the note under the
    # solar wings below for when a surface earns a real shader instead.
    color3f primvars:displayColor = (0.78, 0.62, 0.27)

    uniform bool lunco:vessel = true

    # The flight-control system. It is not something bolted onto the airframe — its
    # `inputs:` ARE the vessel's control surface, the ports the stick writes — so the
    # vessel prim IS the program: `LuncoProgramAPI`, applied above.
    uniform asset lunco:program:sourceAsset = @models/MyLander.mo@
    uniform bool lunco:program:realtimeSafe = true

    # Intent -> port map, so possessing this vessel actually does something.
    def "Controls" ( prepend references = @../control_profiles.usda@</LanderControls> )
    {
    }

    # The altimeter: a downward range-finder that publishes a `range` port.
    # Slung below the rover it will carry, with a clear line to the ground.
    def "Altimeter" ( prepend references = @../sensors/altimeter.usda@</Altimeter> )
    {
        double3 xformOp:translate = (0.0, -3.3, 0.0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }

    # ── Landing legs ── four struts, each ending in a footpad that carries the
    # collider. The pads are what the lander actually stands on.
    def Cube "LegPX"
    {
        double size = 1.0
        double3 xformOp:translate = (3.95, -1.68, 0)
        double3 xformOp:rotateXYZ = (0, 0, 25.0)
        double3 xformOp:scale = (0.15, 7.33, 0.15)
        uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"]
        color3f primvars:displayColor = (0.55, 0.55, 0.57)
        bool physics:collisionEnabled = false
    }
    def Cylinder "PadPX"
    {
        uniform token axis = "Y"
        double radius = 0.4
        double height = 0.08
        double3 xformOp:translate = (5.5, -5.0, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        color3f primvars:displayColor = (0.45, 0.45, 0.48)
        bool physics:collisionEnabled = true
    }
    # ... and the same three more times, mirrored onto -X, +Z and -Z.
    # (Copy `LegPX`/`PadPX`, flipping the translate and rotate signs. The full set
    #  is in `assets/vessels/landers/descent_lander.usda` if you'd rather crib it.)

    # The body's own bulk. Once ANY child carries a collider — the footpads above —
    # the rigid body switches to CHILDREN-ONLY compound mode and drops its own
    # shape. This invisible twin puts the tank back into the compound; without it
    # the lander would balance on four pads and nothing else.
    def Cylinder "Hull" ( prepend apiSchemas = ["PhysicsCollisionAPI"] )
    {
        uniform token axis = "Y"
        double radius = 2.5
        double height = 3.0
        token visibility = "invisible"
        bool physics:collisionEnabled = true
    }
}
```

Notice what's *not* here: no position. A vehicle asset never says where it is.

`lunco:vessel` marks it as something that can be possessed, and the `Controls`
reference says what the keys do once you have it: Space thrusts, W/S pitch, A/D roll,
Q/E yaw. Those land on the model inputs of the same name from Step 2.
`lunco:program:sourceAsset` names your `.mo` file and runs it — a program is a prim,
and here the prim it lives on is the vessel itself, because a lander without its flight
software is not a lander with no autopilot, it is a lander with no engine. And
`references` pulls
the altimeter and the control profile in from the shared libraries — the same
mechanism you're about to use for the lander itself, one level up.

The legs are not decoration, and neither is `Hull`. A prim with
`PhysicsRigidBodyAPI` is a *compound body*: it gathers its collider **children** into
one shape. Give it none and it falls back to its own geometry — fine for a lone
crate. But the moment one child carries a collider (here, the footpads) the body
switches to children-only mode and drops its own shape, so the invisible `Hull` is
what puts the tank back. Miss it and the lander teeters on four dinner plates.

Now mind the three heights, because they have to agree:

- the **pads** sit 5 m below the body centre, so at rest the lander's centre floats
  at y ≈ 5 and there is 3.5 m of clear air under its tank;
- the **rover** (Step 7) hangs from the tank's underside, into exactly that air. A
  legless lander rests with its tank *on the ground*, and slings the rover through
  the floor — the physics fights it and eventually explodes;
- the **altimeter** hangs at −3.3, below the rover but above the pads, so its
  downward ray sees ground rather than the cargo. At rest it reads ≈1.7, which is
  what `rest_altitude` (1.5) is aiming for.

Change one and you must change the others. This is the least glamorous paragraph in
the tutorial and the one that will cost you an evening.

Now dress it. Everything below is **visual only** — `physics:collisionEnabled = false`
throughout, so none of it touches the compound shape you just balanced. Add these as
further children of `MyLander`:

```usda
    # Flat instrument deck on top of the tank.
    def Cylinder "UpperDeck"
    {
        uniform token axis = "Y"
        double radius = 2.6
        double height = 0.15
        double3 xformOp:translate = (0, 1.55, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        color3f primvars:displayColor = (0.6, 0.58, 0.55)
        bool physics:collisionEnabled = false
    }

    # Comms: a mast, a slender spike on top of it, and a high-gain dish off to
    # the side. (Bevy renders an axis-Y cone apex-DOWN, hence the 180 flip.)
    def Cylinder "Antenna"
    {
        uniform token axis = "Y"
        double radius = 0.06
        double height = 1.4
        double3 xformOp:translate = (0.0, 2.35, 0.0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        color3f primvars:displayColor = (0.7, 0.7, 0.72)
        bool physics:collisionEnabled = false
    }
    def Cone "CommSpike"
    {
        uniform token axis = "Y"
        double radius = 0.13
        double height = 1.5
        double3 xformOp:translate = (0.0, 3.6, 0.0)
        double3 xformOp:rotateXYZ = (180.0, 0, 0)
        uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ"]
        color3f primvars:displayColor = (0.82, 0.82, 0.86)
        bool physics:collisionEnabled = false
    }
    def Sphere "AntennaDish"
    {
        double radius = 0.3
        double3 xformOp:translate = (1.3, 1.9, 0.0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        color3f primvars:displayColor = (0.9, 0.9, 0.92)
        bool physics:collisionEnabled = false
    }

    # The engine bell: wide at the exit, narrowing to the throat, with a hot
    # emissive lip. The flame in Step 5 comes out of this.
    def Cone "Nozzle"
    {
        uniform token axis = "Y"
        double radius = 1.35
        double height = 1.9
        double3 xformOp:translate = (0, -2.6, 0)
        double3 xformOp:rotateXYZ = (180.0, 0, 0)
        uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ"]
        color3f primvars:displayColor = (0.14, 0.14, 0.16)
        color3f primvars:emissiveColor = (0.5, 0.18, 0.03)
        bool physics:collisionEnabled = false
    }

    # Solar wings on +/-X, tilted toward the sun. These are the one place we reach
    # for a real shader: `solar_panel.wgsl` draws the cells, busbars and frame
    # procedurally, and every `primvars:*` below is one of its parameters.
    def Cube "SolarWingPX"
    {
        double size = 1.0
        double3 xformOp:translate = (5.4, 1.4, 0)
        double3 xformOp:rotateXYZ = (0, 0, -12.0)
        double3 xformOp:scale = (5.6, 0.06, 3.4)
        uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"]
        color3f primvars:displayColor = (0.06, 0.06, 0.22)
        string primvars:materialType = "shader"
        string primvars:shaderPath = "shaders/solar_panel.wgsl"
        float primvars:cell_rows = 8.0
        float primvars:cell_cols = 14.0
        color3f primvars:cell_color = (0.04, 0.05, 0.28)
        color3f primvars:bus_color = (0.9, 0.9, 0.95)
        color3f primvars:frame_color = (0.15, 0.15, 0.18)
        bool physics:collisionEnabled = false
    }
    # Mirror it onto -X: negate the translate X and the rotate Z.
```

Two things to notice. The hull's warm gold is just `displayColor` — that's
multi-layer insulation foil, and a plain PBR surface renders it fine. The
wings, by contrast, set `materialType = "shader"` and point at a real WGSL shader,
whose knobs are ordinary `primvars` you can tune per instance. Reach for a shader
when the surface has *structure* (cells, busbars); reach for `displayColor` when it
just has a colour.

Beware one trap: `shaderPath` must name a whole shader with a `@fragment` entry
point. Point it at a shader *library* like `pbr_lit.wgsl` (which only exports
functions) and you build an invalid render pipeline — the viewport blinks, the log
fills with validation spam, and nothing tells you why.

Now drop it into the scene. Inside the `Mission` prim in `my_mission.usda`:

```usda
    def Cylinder "Lander" (
        prepend references = @../../vessels/landers/my_lander.usda@</MyLander>
    )
    {
        double3 xformOp:translate = (0, 60.0, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }
```

That's the whole scene-side cost of a lander: *what* it is, and *where* it starts.
Reload — a lander appears at 60 m and drops like a rock, because nothing has
connected the controller to the body yet. That's Step 4.

---

## Step 4 — Wire the controller to the world

A model that nobody listens to changes nothing. Wiring is native USD: an
`inputs:<port>.connect` on the prim, pointing at whatever publishes that value.

Add these lines to the `MyLander` prim (in `my_lander.usda`, not the scene):

```usda
    # The model's thrust becomes a force on the body — along the lander's OWN +Y,
    # not the world's. `force_local_*` is rotated by the body's attitude at apply
    # time, so when you tilt the lander the engine tilts with it and you fly it
    # like a rocket. Wire it to `force_y` instead and the thrust always points at
    # the sky, however far over you lean.
    float inputs:force_local_y.connect = </MyLander.outputs:thrust>

    # Attitude, from the pilot's stick. World-frame torques (N.m).
    float inputs:torque_x.connect = </MyLander.outputs:torque_x>
    float inputs:torque_y.connect = </MyLander.outputs:torque_y>
    float inputs:torque_z.connect = </MyLander.outputs:torque_z>

    # The body tells the controller how fast it's falling, and how heavy it is.
    float inputs:descent_rate.connect = </MyLander.outputs:velocity_y>
    float inputs:vehicle_mass.connect = </MyLander.outputs:mass>

    # The altimeter tells it how high it is. A CROSS-PRIM edge: a different prim.
    float inputs:altitude.connect = </MyLander/Altimeter.outputs:range>

    # AUTHORITY. `piloted` reads 1 whenever any session possesses this vessel.
    # This is the line that makes the gate in Step 2 work.
    float inputs:piloted.connect = </MyLander.outputs:piloted>

    # Publish the model's throttle back onto the entity, so anything downstream
    # (the flame in Step 5, telemetry, the Inspector) can read it as a port.
    float inputs:throttle.connect = </MyLander.outputs:throttle>
```

Read each one as "this input is fed by that output". The self-referencing ones
aren't a mistake: a vessel prim publishes its own *physical* state as outputs
(`velocity_y`, `mass`, `piloted`), and consumes the *model's* outputs as inputs.
Body → model → body, once per timestep. That round-trip is co-simulation, and
[tutorial 03](03-cosim.md) takes it apart properly.

Three details that matter:

- **Every path is asset-local** — `</MyLander...>`, not `</Mission/Lander...>`.
  Authoring them against the asset's own root is what lets the wiring survive the
  reference arc: USD rebases each target onto `/Mission/Lander` when the scene pulls
  the vehicle in. Write scene paths here and the vehicle only ever works in one scene.
- **`piloted` is derived, never authored.** Nothing writes it. Possess the vessel
  and it reads 1; release it and it reads 0. Authority has exactly one source.
- **The pilot's stick needs no wire.** `external_throttle` is fed by the `Controls`
  profile you referenced in Step 3, which maps the `action` intent straight onto a
  port of that name. Don't be tempted to `.connect` it to anything — wiring it to the
  model's own `throttle` output would feed the engine its own exhaust, and the moment
  you possessed the lander the throttle would latch wherever it happened to be.

Reload and watch: the lander eases itself down and settles on the ground. It's
flying on its own controller.

Want to retune it without restarting? From any script, `set(lander, "kv", 2.0)`
changes the gain live. Anything you marked `input` in the model is adjustable on the
fly — and the Inspector will let you drag it.

---

## Step 5 — Give it a flame

A rocket with no flame looks dead. Let's add one that grows and shrinks with the
throttle and flickers like real fire.

The trick: the flame is just a cone that a tiny script resizes every frame based on
the `throttle` signal. Because it reads the engine's *actual* output, it honestly
shows nothing when the engine is off or out of fuel — even if you're mashing Space.

Add two cones as children of `MyLander` — a soft outer plume and a hot inner core:

```usda
    def Cone "Flame"
    {
        uniform token axis = "Y"
        double radius = 1.0
        double height = 1.0
        double3 xformOp:translate = (0, -3.5, 0)
        double3 xformOp:rotateXYZ = (180.0, 0, 0)
        double3 xformOp:scale = (0.02, 0.02, 0.02)
        uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"]
        color3f primvars:displayColor = (0.5, 0.16, 0.02)
        color3f primvars:emissiveColor = (3.0, 1.0, 0.12)
        float primvars:displayOpacity = 0.4
        bool physics:collisionEnabled = false

        def LuncoProgram "Plume"
        {
            uniform asset lunco:program:sourceAsset = @scenarios/flame.rhai@
            custom float lunco:param:wmax = 1.05
            custom float lunco:param:lmax = 3.6
            custom float lunco:param:flick = 1.0
        }
    }
    def Cone "FlameCore"
    {
        uniform token axis = "Y"
        double radius = 1.0
        double height = 1.0
        double3 xformOp:translate = (0, -3.4, 0)
        double3 xformOp:rotateXYZ = (180.0, 0, 0)
        double3 xformOp:scale = (0.02, 0.02, 0.02)
        uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"]
        color3f primvars:displayColor = (0.95, 0.7, 0.2)
        color3f primvars:emissiveColor = (6.0, 3.5, 0.9)
        float primvars:displayOpacity = 0.85
        bool physics:collisionEnabled = false

        def LuncoProgram "Plume"
        {
            uniform asset lunco:program:sourceAsset = @scenarios/flame.rhai@
            custom float lunco:param:wmax = 0.5
            custom float lunco:param:lmax = 2.5
            custom float lunco:param:flick = 0.5
        }
    }
```

A few choices worth a sentence each. `displayOpacity` makes the cones see-through.
The emissive colours are *bigger than 1* on purpose — that makes them glow like a
light source instead of a dull painted surface, so the sun can't wash your orange
flame into beige. And notice both cones carry their own `LuncoProgram` prim pointing at
the **same** script, each with its own typed parameters — `lunco:param:wmax` /
`lunco:param:lmax` are how wide and long each gets, `lunco:param:flick` how much it
flickers. A parameter is just an attribute on the program prim, and `param(me, "wmax",
1.0)` reads it. That's how one script drives two different-looking flames: each cone's
program reads its own numbers.

`scenarios/flame.rhai` already ships with the engine, and it's short enough to read
in full:

```rhai
// Climb up the parents until we find whoever publishes a `throttle` (the engine).
fn nearest_throttle(me) {
    let cur = parent(me);
    let hops = 0;
    while cur != () && hops < 5 {
        let t = get(cur, "throttle");
        if t != () { return t; }
        cur = parent(cur);
        hops += 1;
    }
    0.0
}

fn on_tick(me) {
    let t = nearest_throttle(me);
    if t == () { t = 0.0; }
    t = clamp(t, 0.0, 1.0);

    // A gentle, smoothed flicker so it shimmers instead of strobing.
    let target = 0.8 + rand() * 0.4;
    if this.flick == () { this.flick = 1.0; }
    this.flick = this.flick * 0.7 + target * 0.3;

    // Read this cone's own size/flicker settings.
    let wmax  = param(me, "wmax", 1.0);
    let lmax  = param(me, "lmax", 3.0);
    let depth = param(me, "flick", 1.0);
    let fl = 1.0 + (this.flick - 1.0) * depth;

    let width = (0.28 + 0.72 * t) * wmax;
    let len = t * lmax * fl;
    if len < 0.02 { len = 0.02; }
    set(me, "Transform.scale", [width, len, width]);
}
```

`on_tick` runs every frame for any prim that carries a program. `me` is the prim running
it, and `this` is a little scratchpad that survives between frames (we keep the
smoothed flicker there). `param(me, "wmax", 1.0)` reads the `lunco:param:wmax` attribute
you authored — that's the clean way to give a reusable script per-instance settings.

Reload and watch the descent: a flickering plume that swells under hard braking and
dies to nothing the instant the engine cuts.

---

## Step 6 — Warn about low fuel, and notice the landing

We want a warning when propellant runs low. The fuel level (`m_prop`) is a signal
the model already publishes — we just need to fire an *event* when it crosses a line.

Rather than poll the fuel every frame in a script, declare the thresholds right on
the vehicle and let the engine watch them. Add to the `MyLander` prim:

```usda
    # One prim per rule: the port, the comparison, the threshold and the name.
    def LuncoPortEvent "LowFuel"
    {
        uniform token lunco:event:port = "m_prop"
        uniform token lunco:event:op = "lt"
        double lunco:event:threshold = 200.0
        uniform token lunco:event:emit = "lander_low_fuel"
    }
    def LuncoPortEvent "Depleted"
    {
        uniform token lunco:event:port = "m_prop"
        uniform token lunco:event:op = "le"
        double lunco:event:threshold = 0.5
        uniform token lunco:event:emit = "lander_depleted"
    }

    # And the supervisor that reacts to them — bolted on, so a child program prim.
    def LuncoProgram "Supervisor"
    {
        uniform asset lunco:program:sourceAsset = @scenarios/my_mission/lander_supervisor.rhai@
    }
```

Read `LowFuel` as "when `m_prop` drops below 200, fire `lander_low_fuel`"; `Depleted`
says the same at near-zero. Each fires once when it crosses, and re-arms if the value
climbs back. This is exactly the kind of "watch a signal for a moment" job that belongs
outside the model (remember the warning in Step 2). Every part of the rule is a typed
property, so nothing is hiding in a string the type system can't see.

Now the supervisor. Create `assets/scenarios/my_mission/lander_supervisor.rhai`:

```rhai
fn on_tick(me) {
    // Touchdown is "low AND slow" — two things at once, so it's easiest to spot
    // here and announce as our own event for the mission to wait on. Emits once.
    if this.touchdown != true && __settled(me) {
        this.touchdown = true;
        emit("lander_touchdown");
    }
}

fn on_event(me, evt) {
    if evt.name == "lander_low_fuel" {
        notify_kind("Lander low on fuel.", "warn");
    } else if evt.name == "lander_depleted" {
        notify_kind("Propellant depleted.", "warn");
    }
}

// Low altitude (our own altimeter) plus a near-zero vertical rate. Entity names are
// full prim paths, so `name(me)` finds our sensor wherever the scene put us.
fn __settled(me) {
    let alt_ent = find(name(me) + "/Altimeter");
    if alt_ent < 0 { return false; }          // find() returns -1 when absent
    let alt = get(alt_ent, "range");
    if alt == () || alt >= 2.0 { return false; }
    let vy = get(me, "velocity_y");
    vy != () && vy > -0.2 && vy < 0.2
}
```

A prim can happily carry both a model and a script — the model does the flying, the
script supervises. Note that the script never names `/Mission/Lander`: it resolves
its own altimeter relative to itself, so it rides along into any scene.

`notify(msg)` and `notify_kind(msg, kind)` pop a message onto the screen (`kind` is
`info`, `success`, `warn`, or `error`). We'll use them all over the mission.

Notice the two styles side by side: fuel is a clean single-value threshold, so it's
declared on the prim; touchdown is a compound "low and slow" condition, so a script
watches it and `emit`s its own event. Both end up as events the rest of the mission
can wait for.

---

## Step 7 — Bring a rover, bolted on for the ride

The lander carries a rover down and drops it. Reference a ready-made rover into the
scene (`my_mission.usda`) and clamp it to the lander with a fixed joint:

```usda
    def Xform "SkidRover" ( prepend references = @../../vessels/rovers/skid_rover.usda@</SkidRover> )
    {
        double3 xformOp:translate = (0, 58.35, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]

        def LuncoProgram "Autopilot"
        {
            uniform asset lunco:program:sourceAsset = @scenarios/my_mission/rover_autopilot.rhai@
        }
    }

    def PhysicsFixedJoint "LanderRoverJoint"
    {
        rel physics:body0 = </Mission/Lander>
        rel physics:body1 = </Mission/SkidRover>
        point3f physics:localPos0 = (0, -1.5, 0)
        point3f physics:localPos1 = (0, 0.15, 0)
    }
```

Same pattern as the lander: a vehicle arrives by `references`, the scene supplies
only its starting pose. `localPos0 = (0, -1.5, 0)` bolts the rover to the underside
of the lander's tank — into the clearance the legs bought you in Step 3.

The joint holds the two together through the descent, and the mission breaks it with
`detach_joint(...)` **only after touchdown** (Step 9 waits on the `lander_touchdown`
event before it detaches). That ordering is not cosmetic: cut the rover loose in
flight and it falls from altitude; leave it bolted on and it never drives. Land
first, then release — the rover drops the last metre onto the regolith and rolls
away. We point it at an autopilot script now and write that in Step 10.

---

## Step 8 — Plant the waypoints

The rover's job is to visit three spots. A glowing marker with an invisible trigger
bubble already exists at `assets/vessels/markers/waypoint.usda`; it looks like this:

```usda
def Xform "WaypointMarker"
{
    def Sphere "Dome"
    {
        double radius = 2.5
        color3f primvars:displayColor = (0.2, 0.95, 0.5)
        color3f primvars:emissiveColor = (0.12, 0.85, 0.42)
        float primvars:displayOpacity = 0.28
        bool physics:collisionEnabled = false
    }

    def Sphere "Zone" ( prepend apiSchemas = ["PhysicsCollisionAPI"] )
    {
        double radius = 4.0
        double3 xformOp:translate = (0, 1.0, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        bool physics:collisionEnabled = true
        token visibility = "invisible"
        custom string lunco:triggerZone = "waypoint"
    }
}
```

The dome is just for show (no collider). The `Zone` is the clever bit:
`lunco:triggerZone` turns it into a sensor that doesn't block anything but *fires an
event* — `enter:waypoint` — the moment something drives into it. The event carries
who entered and which sensor fired, so a script can wait on one specific gate with
`wait_for_from("enter:waypoint", "/Mission/RoverTarget2/Zone")` and tell it apart
from its identical siblings.

One honest caveat, and it's why the mission below doesn't use those events: this
rover's wheels are raycasts rather than solid colliders, so its chassis doesn't
reliably overlap the bubble. Trigger zones are the right tool for a walking
astronaut or a solid crate; for this rover we read the distance instead. Reach for
whichever actually fires.

Drop three markers into the scene:

```usda
    def Xform "RoverTarget1" ( prepend references = @../../vessels/markers/waypoint.usda@</WaypointMarker> )
    { double3 xformOp:translate = (14, 0, 9); uniform token[] xformOpOrder = ["xformOp:translate"] }
    def Xform "RoverTarget2" ( prepend references = @../../vessels/markers/waypoint.usda@</WaypointMarker> )
    { double3 xformOp:translate = (-11, 0, 16); uniform token[] xformOpOrder = ["xformOp:translate"] }
    def Xform "RoverTarget3" ( prepend references = @../../vessels/markers/waypoint.usda@</WaypointMarker> )
    { double3 xformOp:translate = (5, 0, -15); uniform token[] xformOpOrder = ["xformOp:translate"] }

    # The touchdown target: a landmark for the pilot, not a wire. The GNC descends
    # on its altimeter and never reads this — it just marks the spot to aim at.
    def Xform "LandingLocation" ( prepend references = @../../vessels/markers/landing_location.usda@</LandingLocationMarker> )
    { double3 xformOp:translate = (0, 0, 0); uniform token[] xformOpOrder = ["xformOp:translate"] }
```

---

## Step 9 — Write the mission

This is the conductor: land, drop the rover, then run the waypoint course. It's a
non-physical prim that holds the storyline and belongs to the *scene*, not to any
vehicle — which is why the lander and rover know nothing about it.

Add it to `my_mission.usda`:

```usda
    def Scope "Scenario"
    {
        custom string lunco:scenario = "my-surface-ops"

        def LuncoProgram "Mission"
        {
            uniform asset lunco:program:sourceAsset = @scenarios/my_mission/mission.rhai@
        }
    }
```

Then create `assets/scenarios/my_mission/mission.rhai`. The mission is written as a
`task` — a list of steps the engine walks through for you, one after another. Each
step either *does* something once, *waits* for some time, or *waits* for a condition
or an event.

```rhai
fn show_wp(n, on) {
    let s = if on { 1.0 } else { 0.0 };
    set(find("/Mission/RoverTarget" + n + "/Dome"), "Transform.scale", [s, s, s]);
}

/// True once the rover is within `r` metres of waypoint `n`.
fn at_wp(n, r) {
    let t = world_pos(find("/Mission/RoverTarget" + n));
    if t == () { return false; }
    arrived(find("/Mission/SkidRover"), t, r)
}

fn task(me) {
    seq([
        once(|m| {
            follow(find("/Mission/Lander"));           // ride the camera down
            show_wp(2, false); show_wp(3, false);      // only the first dome shows
            notify_kind("Powered descent - the GNC is flying the lander down.", "info");
        }),

        // Hold here until the lander's supervisor tells us it touched down.
        wait_for("lander_touchdown"),
        once(|m| notify_kind("Touchdown.", "success")),

        wait(1.5),                                     // brief settle (sim seconds)
        once(|m| { detach_joint(find("/Mission/LanderRoverJoint")); notify("Releasing the rover..."); }),
        wait(2.0),                                     // let the rover fall clear

        once(|m| {
            emit("rover_deployed");                    // wakes the autopilot (Step 10)
            notify_kind("Rover deployed - autopilot driving. Click the rover (or press F) to take over.", "success");
        }),

        // The course. Reaching a gate hides it, reveals the next, and announces
        // itself on the bus so anything watching can react.
        wait_until(|m| at_wp(1, 5.0)),
        once(|m| { show_wp(1, false); show_wp(2, true); emit("waypoint_1_reached");
                   notify_kind("Waypoint 1 reached (2 of 3).", "success"); }),
        wait_until(|m| at_wp(2, 5.0)),
        once(|m| { show_wp(2, false); show_wp(3, true); emit("waypoint_2_reached");
                   notify_kind("Waypoint 2 reached (3 of 3).", "success"); }),
        wait_until(|m| at_wp(3, 5.0)),
        once(|m| { show_wp(3, false); emit("course_complete");
                   notify_kind("All waypoints reached - course complete!", "success"); }),
    ])
}
```

A few of the verbs you just used: `find("/path")` looks up a prim by its scene path;
`detach_joint` breaks the joint from Step 7; `wait(secs)` pauses the story (in
sim-seconds); `wait_for(name)` parks until an event fires; `wait_until(pred)` parks
until a condition reads true.

**`follow`, not `possess`.** This is the trap the whole tutorial has been building
toward. `follow(lander)` attaches a chase camera and nothing else — the lander stays
unpossessed, its `piloted` port reads 0, and the GNC from Step 2 keeps flying it. If
you'd written `possess(lander)` instead, `piloted` would snap to 1, the gate would
switch to the pilot's stick, and your beautiful descent controller would go silent
while the lander dropped like a stone. Possession *means* "a pilot has this", so
possess a vehicle only when you actually intend to fly it.

Notice, too, that this script doesn't drive anything. It watches for real things
happening and announces them (`rover_deployed`, `waypoint_1_reached`,
`course_complete`). Who *listens* is not its problem — that's the next step, and it's
also how the in-app lesson layers a student checklist over this same mission without
touching a line of it.

Reload and watch the whole arc play out — except the rover just sits there, because
we haven't taught it to drive yet.

---

## Step 10 — Let the rover drive itself, and hand over cleanly

Finally, the autopilot. It should drive the rover from gate to gate on its own, and
the moment the player wants the wheel, get out of the way.

"The moment the player wants the wheel" is where autopilots usually go wrong, so
let's be precise. The player is asking for the rover if they possess the rover.
They are *not* asking for the rover when they press W while flying the lander — that
W belongs to the lander. An autopilot that grabs possession on any keypress will rip
the camera off the lander mid-descent, which is a genuinely baffling thing to
experience. Authority is a question you *ask*, never one you assume.

Create `assets/scenarios/my_mission/rover_autopilot.rhai`:

```rhai
fn wp_pos(i) { world_pos(find("/Mission/RoverTarget" + i)) }

/// True if a HUMAN drives `id`. `controller(id)` names the driver's role —
/// "AiAgent" for an autopilot, "Owner"/"Operator" for a person — so this is the
/// human-vs-AI test, not a bare "is anyone driving". find() gives -1 when absent.
fn human_drives(id) {
    if id < 0 { return false; }
    let role = controller(id);
    role != () && role != "AiAgent"
}

/// Is the player at the controls of some OTHER vehicle? Their keys are its keys.
fn __player_flies_elsewhere(me) {
    let lander = find("/Mission/Lander");
    lander != me && human_drives(lander)
}

/// The side-effects of standing down. `this.manual` is NOT set here — see below.
fn __stand_down(me) {
    brake(me);
    notify_kind("Manual control - autopilot off.", "info");
}

fn on_tick(me) {
    if this.active != true { return; }        // not deployed yet
    if this.manual == true { return; }        // player has the wheel
    // They possessed us themselves (clicked, or pressed F). Yield — and possess
    // nothing on their behalf: they already own it.
    if human_drives(me) { this.manual = true; __stand_down(me); return; }

    if this.i == () { this.i = 1; }
    if this.i > 3 { brake(me); return; }      // course done

    let target = wp_pos(this.i);
    if target == () { return; }
    if nav_to(me, target, 0.7, 4.0) {         // steer there; true once arrived
        this.i += 1;
    }
}

fn on_event(me, evt) {
    if evt.name == "rover_deployed" {
        this.active = true; this.i = 1;
        return;
    }
    if !evt.name.starts_with("key:") { return; }
    if this.active != true || this.manual == true { return; }
    if __player_flies_elsewhere(me) { return; }   // that key was for the lander
    // Nothing else is possessed, so the key IS a takeover request. Give them the
    // rover so their keys reach it, and stand down.
    possess(me);
    this.manual = true;
    __stand_down(me);
}
```

Driving is a moment-to-moment thing — steer a little this frame, a little the next —
so it lives in `on_tick`. `nav_to(rover, target, speed, radius)` does the steering
for you and returns `true` once it's arrived (and brakes); then we move on to the
next waypoint.

One rhai rule bites here, so learn it now: **`this` is bound only inside the hook
the engine calls** — `on_tick`, `on_event` — and *not* inside functions those hooks
call. That's why `__stand_down` brakes and notifies but leaves `this.manual = true`
to its callers. Move that assignment into the helper and the autopilot will cheerfully
keep driving forever, having "stood down" into a `this` that nobody reads.

When the autopilot *turns on and off*, though, those are events and state reads. It
wakes on the `rover_deployed` event the mission fired (so it doesn't fight the
descent while still bolted to the lander), and it stands down the first time a human
holds the rover. There are two ways for that to happen, and both funnel through
possession:

- the player possesses the rover directly (clicks it, presses F) — `on_tick` sees a
  human owner and yields, possessing nothing;
- the player presses a drive key with nothing else possessed — the autopilot
  possesses the rover *for* them as a convenience, then yields.

Every key you hit shows up as an event named `key:<something>`, so "any key" is just
checking the name starts with `key:` — but only after `__player_flies_elsewhere`
confirms the key wasn't meant for someone else.

Reload one more time. The lander flies down, the rover drops and drives the course
on its own, the domes light up one by one — and the moment you click it, the rover is
yours. Click the *lander* instead and you'll fly the lander, with the rover trundling
along on autopilot behind you, exactly as it should.

---

## You built a mission

Step back and look at what's there: a vehicle asset with a real control law, wired
to its own sensors, reusable in any scene; a flame driven by its actual engine
output; model-driven warnings; a multi-stage mission that waits on physical events
and announces its own; and an autopilot that yields to a human without ever stealing
from one. None of it needed engine changes — just a scene, a vehicle, a model, and a
handful of scripts.

The shape to carry away is the layering. **Vehicles** own their physics, control
law, and wiring, and know nothing of any mission. **Scenes** place vehicles and own
the storyline. **Missions** announce what happened; they never reach into a vehicle's
state. **Authority** is possession, asked and never assumed. Follow those four and
new missions compose out of old vehicles for free.

From here, good next moves: add a battery or thermal model as a second `.mo` on the
rover and watch its ports; turn "take a photo at the rock" into another trigger zone;
or give each waypoint a time limit and fail the mission if it's missed.

---

### A short cheat-sheet for when something misbehaves

- The whole script silently stops working and the log repeats a compile error?
  Check your strings — rhai doesn't accept `\u{...}` escapes. Plain text only.
- A vehicle falls through the floor? Check the ground actually has
  `PhysicsCollisionAPI` and `physics:collisionEnabled`. A collider prim with no
  rigid-body ancestor becomes static geometry wherever it sits in the tree.
- A vehicle drops like a rock with the model attached? Its `.connect` wiring is
  missing, or it names scene paths (`</Mission/Lander...>`) instead of asset-local
  ones (`</MyLander...>`), so nothing rebased through the reference.
- Touchdown fires while the lander is still in the air, or never fires at all? Check
  where the altimeter is mounted against the height the hull rests at. `rest_altitude`
  must equal the range the sensor reads with the vehicle sitting on the ground — and
  the sensor must hang below any cargo, or it ranges the cargo instead of the ground.
- Physics goes berserk at touchdown, or a slung payload sinks into the floor? The
  carrier has no ground clearance. Its legs must hold the attachment point above the
  ground by more than the payload is tall.
- A vehicle stands on its footpads alone, tank clipping through everything? It went
  children-only compound when the pads got colliders. Add the invisible `Hull` twin.
- Your self-flying vehicle went inert the moment a script touched it? Something
  possessed it. `piloted` is 1, so the authority gate handed control to a pilot who
  isn't pressing anything. Use `follow()` to watch, `possess()` only to fly.
- A flame looks beige instead of fiery? Its emissive colour is probably ≤ 1 — push
  it above 1 so it glows instead of being lit by the sun.
- A model line with `if … else if …`? Flatten it into separate single `if`s,
  `min`/`max`, or arithmetic; chained `else if` doesn't translate cleanly.
- Need a model to *decide* something from a wired-in value? Don't — declare a
  `LuncoPortEvent` threshold prim instead (Step 6).
- A trigger zone never fires? Check what's entering it. Raycast-wheeled rovers don't
  reliably overlap sensors; read a distance instead (Step 8).
- Possessed the vehicle, and only Space does anything? Your model has no `pitch` /
  `roll` / `yaw` inputs, so the `Controls` profile is writing ports nobody reads.
  Ports bind by NAME — a typo is silence, not an error.
- Tilting the vehicle doesn't steer the thrust? You wired `force_y` (world up)
  instead of `force_local_y` (the vehicle's own up).
- Reading the engine's *real* state? Read its output signal (`throttle`), not the
  player's input.

For the complete list of script verbs — sensing, math, vehicle control, tools,
networking — see [`../scripting-guide.md`](../scripting-guide.md). For the reasoning
behind how scenarios are put together, see
[`../architecture/34-scenario-and-multidomain.md`](../architecture/34-scenario-and-multidomain.md).

## Related

- **Play the interactive version**: the in-app *Lander & Rover Mission* lesson
  (🎓 Tutorials menu in the sandbox) walks the same mission with on-screen coaching.
  It watches the very events you emitted in Step 9 and turns them into a checklist.
- **Next walkthrough**: [02 — Author your own controller](02-authoring-a-controller.md) — build the lander's GNC yourself.
- **Then**: [03 — Cosim: when a model flies physics](03-cosim.md) — the wiring from Step 4, in depth.
- **Reference skills**: [build-usd-scene](../../skills/build-usd-scene/SKILL.md) (the world),
  [author-scenario](../../skills/author-scenario/SKILL.md) (the mission logic),
  [compose-multidomain-twin](../../skills/compose-multidomain-twin/SKILL.md) (wiring it all together),
  [inspect-simulation](../../skills/inspect-simulation/SKILL.md) (watching it run). Full index: [skills/](../../skills/README.md).
