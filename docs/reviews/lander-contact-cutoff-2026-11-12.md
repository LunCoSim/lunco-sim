# Lander contact cutoff and landing stability — 2026-11-12

The production Griffin surface mission reached gentle first-pad contact but
kept its main engines running. `Lander.engine_cutoff_contact` reused the final
settled-landing angular-rate limit, 0.005 rad/s. At first contact the body rate
was approximately 0.021 rad/s, with negligible ground speed and slow descent.
The lightly loaded gear could not settle before horizontal drift exceeded the
target gate. The existing guidance recovery then commanded a climb. A baseline
300-second stability run ended INCONCLUSIVE without qualified touchdown.

## Ownership and change

The reusable Modelica airframe now has an independent live input,
`engine_cutoff_angular_speed_rad_s`, default 0.05 rad/s (about 2.9 degrees/s).
This is a simulation study setting, not a Griffin flight ICD limit. Like the
existing cutoff speed limits, it can be authored on a vehicle's USD input
surface. It bounds residual rotation when transferring support to the gear;
it does not declare that the vehicle is settled.

Cutoff still requires native pad contact, upright attitude, ground speed no
greater than 0.08 m/s, descent-speed magnitude no greater than 0.20 m/s, and
the GNC landing-position gate. Fast, airborne, tipped and off-target states do
not gain cutoff permission. The existing event supervisor latches the accepted
target-qualified transition. Rate damping remains active during gear settling.
Final handoff retains the original four-pad, 0.005 rad/s, linear-speed and
suspension-rate predicates; it retires the remaining RCS demands.

Continuous control and measured predicates remain in Modelica, native contact
and rigid-body motion in Avian, target qualification in the existing GNC, and
event latching in the existing supervisor. There is no new script controller,
forced pose, artificial contact, altered terrain, or physics-cadence change.

## Production validation

Evidence: `.cache/perf/plume-completion/` in the simulator checkout.

- `contact-contracts-final/`: 78 authored Rhai checks against production
  Modelica experiments. Covers partial-pad cutoff without settled handoff,
  both inclusive rate boundaries, a stricter authored cutoff rate, unsafe
  contact rejection, moving struts, retained damping and zero commands after
  handoff. Driver: `scripts/api/test_lander_contact.py --output <directory>`.
- `landing-stability-{fixed,replay}.log`: two fresh single-thread production
  runs of the Twin's unchanged `tests/griffin_surface_ops_contract.usda`, both
  PASS at tick 4950 / 82.5 simulated seconds. Touchdown tick 1342; 361 samples
  over 60.1333 seconds after touchdown, every sample with all four pads in
  contact, no unready samples, maximum drift 0.0201222 m, minimum up-axis
  projection 0.9962432, maximum ground speed 0.0111598 m/s and maximum angular
  speed 0.00979892 rad/s. Physical metrics match exactly between runs. Only
  virtual elapsed time differs due to startup scheduler time; the integer
  simulation clock and the stability horizon agree.
- `gnc-handoff.log`: existing Twin GNC disconnect/reconnect gate PASS, 22 checks.
- `landing-cutoff/`: visible production scene, body-mounted leg camera,
  authoritative port/physics samples and screenshots. Engine cutoff occurs
  near 21.6 seconds; all pads contact before final handoff near 22.7 seconds.
  The lander remains supported through 50 seconds. `landing-settled/` retains
  the subsequent visible settled-actuation and exterior-view check: all four
  pads qualify, throttle and all three torque demands are zero, all seven
  engines have decayed below 1e-6 N, autonomous authority remains selected, and
  readiness is clear. An
  intervening `landing-final/` run received a native `thrust` press at tick
  3484 / 58.0667 seconds after a scene-selection click; its subsequent manual
  liftoff is excluded from autonomous acceptance.

Runs use the task-owned native executable with source assets from this checkout
and the current Twin, the real NOBILE03 terrain and complete attached assembly.
The retained executable identifies as `99249918-dirty`; its SHA256 is
`292ed92c75a4f34be681ca605a71625568e05b965d7c338afb6c0a437ea14c98`.
No Rust rebuild is needed for this Modelica source change, but an installed
application must load the updated shared `assets/models/Lander.mo` and reload
the scene. These receipts cover the study landing and its stability horizon;
they do not qualify flight hardware or the deferred restrained rover descent.
