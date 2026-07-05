# Tutorial 01 — Build a Lander → Rover Mission

In this tutorial you'll build a small lunar mission from scratch. A lander flies
itself down to the surface on a glowing engine plume, settles on its legs, and
safes its engine. It then releases a rover, which drives itself through a course
of glowing waypoints — until you grab the controls and take over. Along the way
the screen narrates each phase.

You'll write three kinds of files, all under `assets/`:

- a **`.usda`** scene — the world: ground, lander, rover, lights, markers;
- a **`.mo`** Modelica model — the lander's flight control law;
- a few **`.rhai`** scripts — the mission logic and behaviours.

Edit a file, reload the scene, watch it change. That's the whole loop. Let's go.

You can run your work at any point with:

```
cargo run -p lunco-sandbox --bin sandbox -- --api 4101 --scene scenes/sandbox/my_mission.usda
```

---

## A quick mental model

Before the first file, one distinction that makes everything else click.
Things in the sim are either **signals** or **events**.

A **signal** is a value that always has a current reading — altitude, throttle,
fuel remaining, gravity. You *read* a signal whenever you want it. Models publish
signals; scripts read them with `get(...)`.

An **event** is a thing that happens once — a touchdown, a rover crossing a
finish line, a key press, the fuel dropping below a line. You don't poll for
events; you *react* to them. Scripts fire events with `emit(...)` and listen with
`on_event` / `wait_for`.

Whenever you're unsure where some logic belongs, ask: "is this a value I read, or
a moment I react to?" Signal or event. Keep them straight and the rest is easy.

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

    def Xform "Avatar"
    {
        string lunco:avatar = "true"
        string lunco:cameraMode = "freeflight"
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

A USD scene is a tree of *prims*. The ground is a big flat cube with a collider
but no rigid body, so it stays put and nothing falls through it. `lunco:avatar`
marks the camera the player looks through. Run it now — you'll see an empty grey
plain. Not much yet, but it's solid ground.

A couple of physics rules worth knowing as you go: a prim with a collider and
**no** rigid body is *static* (the ground). Add a rigid body and a mass and it
becomes *dynamic* — it falls and reacts to forces. That's what the lander will be.

---

## Step 2 — Teach the lander how to fly

Our lander shouldn't just drop — it should *fly itself down*, easing onto its
legs like a real one. That flight law is a small PID controller, and the natural
place for control laws is Modelica.

Create `assets/models/Lander.mo`:

```modelica
model Lander
  parameter Real vehicle_mass = 2000.0;
  parameter Real max_thrust = 60000.0;
  parameter Real v_e = 2900.0;            // exhaust velocity (for fuel book-keeping)

  // Inputs — anything you might want to change while it runs (gains, set-point,
  // engine on/off). Keep these as `input`, not `parameter`, so you can retune
  // them live.
  input Real target_altitude = 3.0;
  input Real kp = 1.2;
  input Real kd = 2.5;
  input Real ki = 0.4;
  input Real engine_enable = 1.0;
  input Real manual = 0.0;
  input Real manual_throttle = 0.0;

  // These three come from outside: the body's height and speed, and gravity.
  input Real altitude = target_altitude;
  input Real descent_rate = 0.0;
  input Real g = 1.62;

  Real i_err(start = 0);
  Real m_prop(start = 2000.0);            // propellant remaining (kg)
  Real a_cmd; Real pid_raw; Real pid_pos; Real pid_thrust;
  Real thrust;                            // what the engine pushes with
  Real throttle;                          // 0..1, how hard it's firing
equation
  der(i_err) = if engine_enable > 0.5 and (altitude - target_altitude) < 5.0
                  and (altitude - target_altitude) > (-5.0)
               then target_altitude - altitude else 0.0;

  a_cmd = g + kp*(target_altitude - altitude) - kd*descent_rate + ki*i_err;

  pid_raw = vehicle_mass * a_cmd;
  pid_pos = if pid_raw > 0.0 then pid_raw else 0.0;
  pid_thrust = if pid_pos > max_thrust then max_thrust else pid_pos;

  thrust = manual*(manual_throttle*max_thrust) + (1.0 - manual)*engine_enable*pid_thrust;
  throttle = thrust / max_thrust;
  der(m_prop) = -thrust / v_e;
end Lander;
```

Read it top to bottom and it's just a hover controller: take the altitude error,
add some damping on the descent rate, feed-forward gravity, and clamp the result
to a sane thrust. The `throttle` output (0..1) is the engine's "how hard am I
firing right now" — we'll use it for the flame later.

Two things will trip you up if nobody warns you, so here they are:

- **Write clamps and mode-switches as flat `if`s or arithmetic, not nested
  `if/else if`.** The two clamp lines above are single `if … then … else`, and
  the `thrust` line mixes the manual/auto modes with plain multiplication by the
  `manual` (0/1) flag. Nested `if/else if` chains don't translate reliably yet.
- **Don't make decisions inside the model based on the wired-in inputs.** A line
  like `when altitude < 0.2` won't see the live altitude. When you need a
  threshold like that, detect it *outside* the model — Step 5 shows how.

---

## Step 3 — Drop the lander in and connect the controller

Now make the lander a real falling body and hand it to the controller.

Add this inside the `Mission` prim:

```usda
    def Cylinder "Lander" ( prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI", "PhysicsMassAPI"] )
    {
        uniform token axis = "Y"
        double radius = 2.5
        double height = 3.0
        double3 xformOp:translate = (0, 30.0, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        bool physics:rigidBodyEnabled = true
        bool physics:collisionEnabled = true
        float physics:mass = 2000.0

        string lunco:vessel = "true"

        string lunco:modelicaModel = "models/Lander.mo"
        string lunco:simWires = "thrust:force_local_y,height:altitude,velocity_y:descent_rate,gravity_accel:g"
    }
```

`lunco:modelicaModel` attaches your `.mo` file to this body and runs it. The
interesting line is `lunco:simWires` — it's the wiring loom that connects the
controller to the world, one `output:input` pair at a time:

- `thrust:force_local_y` — the model's thrust becomes a force pushing along the
  lander's own "up";
- `height:altitude` and `velocity_y:descent_rate` — the body tells the controller
  where it is and how fast it's falling;
- `gravity_accel:g` — **gravity comes from the environment**, not a number you
  typed. On the Moon that feeds ≈1.62 into the model. Always wire gravity in this
  way; never hard-code it, or your lander will behave wrong the moment it's
  somewhere else.

Reload and watch: the lander appears at 30 m and eases itself down to a 3 m
hover, then settles. It's flying on its own controller.

`lunco:vessel` makes it something you can take control of — click it and hold
Space to fly it by hand. That works because Space just sets the `manual` input
through the very same wiring; manual and automatic control are the same machinery.

Want to retune it without restarting? From any script, `set(lander, "kp", 2.0)`
changes the gain live, and `set(lander, "engine_enable", 0.0)` cuts the engine.
Anything you marked `input` in the model is adjustable on the fly.

---

## Step 4 — Give it a flame

A rocket with no flame looks dead. Let's add one that grows and shrinks with the
throttle and flickers like real fire.

The trick: the flame is just a cone that a tiny script resizes every frame based
on the `throttle` signal. Because it reads the engine's *actual* output, it
honestly shows nothing when the engine is off or out of fuel — even if you're
mashing Space.

Add two cones as children of the `Lander` — a soft outer plume and a hot inner
core:

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
            custom string lunco:params = "wmax=1.05, lmax=3.6, flick=1.0"
            custom string lunco:scriptPath = "scenarios/flame.rhai"
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
            custom string lunco:params = "wmax=0.5, lmax=2.5, flick=0.5"
            custom string lunco:scriptPath = "scenarios/flame.rhai"
        }
```

A few choices worth a sentence each. `displayOpacity` makes the cones see-through.
The emissive colours are *bigger than 1* on purpose — that makes them glow like a
light source instead of a dull painted surface, so the sun can't wash your orange
flame into beige. And notice both cones point to the **same** script but carry
different `lunco:params` — `wmax`/`lmax` are how wide and long each gets, `flick`
how much it flickers. That's how one script drives two different-looking flames:
each cone reads its own numbers.

Now the script. Create `assets/scenarios/flame.rhai`:

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

`on_tick` runs every frame for any prim that has a script. `me` is the prim
running it, and `this` is a little scratchpad that survives between frames (we
keep the smoothed flicker there). `param(me, "wmax", 1.0)` reads a number you
authored on the prim — that's the clean way to give a reusable script
per-instance settings.

Reload and fly the descent: a flickering plume that swells under hard braking and
dies to nothing the instant the engine cuts.

---

## Step 5 — Warn about low fuel (an event from the model)

We want a warning when propellant runs low, and an automatic engine cut when it's
gone. The fuel level (`m_prop`) is a signal the model already publishes — we just
need to fire an *event* when it crosses a line.

Rather than poll the fuel every frame in a script, declare the thresholds right
on the lander and let the engine watch them. Add to the `Lander` prim:

```usda
        custom string lunco:portEvents = "m_prop<200:lander_low_fuel, m_prop<=0.5:lander_depleted"
```

Read it as "when `m_prop` drops below 200, fire `lander_low_fuel`; when it hits
near-zero, fire `lander_depleted`." Each fires once when it crosses, and re-arms
if the value climbs back. This is exactly the kind of "watch a signal for a
moment" job that belongs outside the model (remember the warning in Step 2).

Now a small script reacts to those events. Create
`assets/scenarios/lander_subsystems.rhai`:

```rhai
fn on_tick(me) {
    // Touchdown is "low AND slow" — two things at once, so it's easiest to spot
    // here and announce as our own event for the mission to wait on.
    if this.touchdown != true {
        let pos = world_pos(me);
        let v = velocity(me);
        if pos != () && v != () && pos[1] < 3.4 && v[1] > -0.1 && v[1] < 0.1 {
            this.touchdown = true;
            emit("lander_touchdown");
        }
    }
}

fn on_event(me, evt) {
    if evt.name == "lander_low_fuel" {
        notify_kind("Lander low on fuel.", "warn");
    } else if evt.name == "lander_depleted" {
        set(me, "engine_enable", 0.0);
        notify_kind("Propellant depleted - engine disarmed.", "warn");
    }
}
```

Attach it to the lander with `custom string lunco:scriptPath =
"scenarios/lander_subsystems.rhai"`. A prim can happily carry both a model and a
script — the model does the flying, the script supervises.

`notify(msg)` and `notify_kind(msg, kind)` pop a message onto the screen (`kind`
is `info`, `success`, `warn`, or `error`). We'll use them all over the mission.

Notice the two styles side by side: fuel is a clean single-value threshold, so
it's declared on the prim; touchdown is a compound "low and slow" condition, so a
script watches it and `emit`s its own event. Both end up as events the rest of
the mission can wait for.

---

## Step 6 — Bring a rover, bolted on for the ride

The lander carries a rover down and drops it. Reference a rover and clamp it to
the lander with a fixed joint:

```usda
    def Xform "SkidRover" ( prepend references = @../../vessels/rovers/skid_rover.usda@</SkidRover> )
    {
        double3 xformOp:translate = (0, 28.35, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        custom string lunco:scriptPath = "scenarios/rover_autopilot.rhai"
    }

    def PhysicsFixedJoint "LanderRoverJoint"
    {
        rel physics:body0 = </Mission/Lander>
        rel physics:body1 = </Mission/SkidRover>
        point3f physics:localPos0 = (0, -1.5, 0)
        point3f physics:localPos1 = (0, 0.15, 0)
    }
```

`references` pulls in a ready-made rover so you don't rebuild a vehicle from
scratch. The joint holds the two together during descent; later the mission
breaks it with `detach_joint(...)` and the rover drops free. We point the rover at
an autopilot script now and write it in Step 9.

---

## Step 7 — Plant the waypoints

The rover's job is to visit three spots. Each spot should announce "the rover got
here" by itself, so make a reusable glowing marker with an invisible trigger
bubble around it.

Create `assets/vessels/markers/waypoint.usda`:

```usda
#usda 1.0
( defaultPrim = "WaypointMarker" upAxis = "Y" metersPerUnit = 1 )

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
`lunco:triggerZone` turns it into a sensor that doesn't block anything but *fires
an event* — `enter:waypoint` — the moment the rover drives into it. The event
carries who entered and which sensor fired.

Now drop three of them into the scene. They all use the same name on purpose —
we'll tell them apart by *which* sensor fired, not by giving each a different
name:

```usda
    def Xform "RoverTarget1" ( prepend references = @../../vessels/markers/waypoint.usda@</WaypointMarker> )
    { double3 xformOp:translate = (40, 0, 18); uniform token[] xformOpOrder = ["xformOp:translate"] }
    def Xform "RoverTarget2" ( prepend references = @../../vessels/markers/waypoint.usda@</WaypointMarker> )
    { double3 xformOp:translate = (8, 0, 46); uniform token[] xformOpOrder = ["xformOp:translate"] }
    def Xform "RoverTarget3" ( prepend references = @../../vessels/markers/waypoint.usda@</WaypointMarker> )
    { double3 xformOp:translate = (-34, 0, 30); uniform token[] xformOpOrder = ["xformOp:translate"] }
```

---

## Step 8 — Write the mission

This is the conductor: land, safe the engine, drop the rover, then run the
waypoint course. It's a non-physical prim that holds the whole storyline.

Add it to the scene:

```usda
    def Scope "Scenario"
    {
        custom string lunco:scenario = "rover-surface-ops"
        custom string lunco:scriptPath = "scenarios/rover_surface_ops.rhai"
    }
```

Then create `assets/scenarios/rover_surface_ops.rhai`. The mission is written as a
`task` — a list of steps the engine walks through for you, one after another.
Each step either *does* something once, *waits* for some time, or *waits* for an
event.

```rhai
fn show_wp(n, on) {
    let s = if on { 1.0 } else { 0.0 };
    set(find("/Mission/RoverTarget" + n + "/Dome"), "Transform.scale", [s, s, s]);
}
fn wp_zone(n) { "/Mission/RoverTarget" + n + "/Zone" }

fn task(me) {
    seq([
        once(|m| {
            possess(find("/Mission/Lander"));
            show_wp(2, false); show_wp(3, false);     // only the first dome shows
            notify_kind("Powered descent - the lander is flying itself down.", "info");
        }),

        // Hold here until the lander tells us it touched down.
        wait_for("lander_touchdown"),
        once(|m| {
            set(find("/Mission/Lander"), "engine_enable", 0.0);
            notify_kind("Touchdown. Engine safed.", "success");
        }),

        wait(1.5),
        once(|m| { detach_joint(find("/Mission/LanderRoverJoint")); notify("Releasing the rover..."); }),
        wait(2.0),

        once(|m| {
            possess(find("/Mission/SkidRover"));
            emit("rover_deployed");                    // wakes the autopilot (Step 9)
            notify_kind("Rover deployed - autopilot driving. Press any key to take over.", "success");
        }),

        // The course. Every gate fires "enter:waypoint", so we wait on the
        // SPECIFIC gate's sensor each time, and reveal the next dome on arrival.
        wait_for_from("enter:waypoint", wp_zone(1)),
        once(|m| { show_wp(1, false); show_wp(2, true); notify_kind("Waypoint 1 reached (2 of 3).", "success"); }),
        wait_for_from("enter:waypoint", wp_zone(2)),
        once(|m| { show_wp(2, false); show_wp(3, true); notify_kind("Waypoint 2 reached (3 of 3).", "success"); }),
        wait_for_from("enter:waypoint", wp_zone(3)),
        once(|m| { show_wp(3, false); notify_kind("All waypoints reached - mission complete!", "success"); }),
    ])
}
```

A few of the verbs you just used: `possess` puts the camera and controls on a
vehicle; `find("/path")` looks up a prim by its scene path; `detach_joint` breaks
the joint from Step 6; `wait(secs)` pauses the story (in sim-seconds);
`wait_for(name)` parks until an event fires.

The one to dwell on is `wait_for_from`. Our three gates all shout the same thing,
`enter:waypoint`, so a plain `wait_for("enter:waypoint")` couldn't tell gate 1
from gate 3. `wait_for_from("enter:waypoint", wp_zone(2))` adds "…and only from
*this* sensor." That's how you handle many identical-looking sources: match on
*who* fired, not just *what*. (If you ever need it inside an `on_event` handler
instead, the same information is right there as `evt.source`.)

Reload and watch the whole arc play out — except the rover just sits there,
because we haven't taught it to drive yet.

---

## Step 9 — Let the rover drive itself

Finally, the autopilot. It should drive the rover from gate to gate on its own,
and the instant the player touches a key, get out of the way.

Create `assets/scenarios/rover_autopilot.rhai` (already attached back in Step 6):

```rhai
fn wp_pos(i) { world_pos(find("/Mission/RoverTarget" + i)) }

fn on_tick(me) {
    if this.active != true { return; }     // not deployed yet
    if this.manual == true { return; }     // player has the wheel
    if this.i == () { this.i = 1; }
    if this.i > 3 { brake(me); return; }   // course done

    let target = wp_pos(this.i);
    if target == () { return; }
    if nav_to(me, target, 0.7, 4.0) {      // steer there; true when arrived
        this.i += 1;
    }
}

fn on_event(me, evt) {
    if evt.name == "rover_deployed" {
        this.active = true; this.i = 1;
    } else if evt.name.starts_with("key:") {
        if this.active == true && this.manual != true {
            this.manual = true; brake(me);
            notify_kind("Manual control - autopilot off.", "info");
        }
    }
}
```

Driving is a moment-to-moment thing — steer a little this frame, a little the
next — so it lives in `on_tick`. `nav_to(rover, target, speed, radius)` does the
steering for you and returns `true` once it's arrived (and brakes); then we move
on to the next waypoint.

When the autopilot *turns on and off*, though, those are events. It wakes up on
the `rover_deployed` event the mission fired (so it doesn't fight the descent
while still bolted to the lander), and it stands down on any key press. Every key
you hit shows up as an event named `key:<something>`, so "any key" is just
checking the name starts with `key:`.

Reload one more time. The lander flies down, the rover drops and drives the
course on its own, the domes light up one by one — and the moment you press a key,
the rover is yours.

---

## You built a mission

Step back and look at what's there: a vehicle with a real control law, a flame
driven by its actual engine output, model-driven warnings, a multi-stage mission
that waits on physical events, and an autopilot that yields to a human. None of it
needed engine changes — just a scene, a model, and a handful of scripts.

From here, good next moves: add a battery or thermal model as a second `.mo` on
the rover and watch its ports; turn "take a photo at the rock" into another
trigger zone; or give each waypoint a time limit and fail the mission if it's
missed.

---

### A short cheat-sheet for when something misbehaves

- The whole script silently stops working and the log repeats a compile error?
  Check your strings — rhai doesn't accept `\u{...}` escapes. Plain text only.
- A flame looks beige instead of fiery? Its emissive colour is probably ≤ 1 —
  push it above 1 so it glows instead of being lit by the sun.
- A model line with `if … else if …`? Flatten it into separate single `if`s or
  arithmetic; chained `else if` doesn't translate cleanly.
- Need a model to *decide* something from a wired-in value? Don't — declare a
  `lunco:portEvents` threshold instead (Step 5).
- Reading the engine's *real* state? Read its output signal (`throttle`), not the
  player's input.

For the complete list of script verbs — sensing, math, vehicle control, tools,
networking — see [`../scripting-guide.md`](../scripting-guide.md). For the
reasoning behind how scenarios are put together, see
[`../architecture/34-scenario-and-multidomain.md`](../architecture/34-scenario-and-multidomain.md).

## Related

- **Play the interactive version**: the in-app *Lander & Rover Mission* lesson
  (🎓 Tutorials menu in the sandbox) walks the same mission with on-screen coaching.
- **Next walkthrough**: [02 — Author your own controller](02-authoring-a-controller.md) — build the lander's GNC yourself.
- **Reference skills**: [build-usd-scene](../../skills/build-usd-scene/SKILL.md) (the world),
  [author-scenario](../../skills/author-scenario/SKILL.md) (the mission logic),
  [compose-multidomain-twin](../../skills/compose-multidomain-twin/SKILL.md) (wiring it all together),
  [inspect-simulation](../../skills/inspect-simulation/SKILL.md) (watching it run). Full index: [skills/](../../skills/README.md).
