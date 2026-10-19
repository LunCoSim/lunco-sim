# Conservative propulsion plume review — 2026-10-19

Modelica owns the reduced ideal-gas streamtube, mass/enthalpy closure,
pressure-thrust separation, limiting-reactant combustion and transported phase.
USD owns geometry, connections and the explicit GPU narrowing boundary. WGSL
integrates the emitted field. Existing engine mechanisms suffice; there are no
Rust or dependency changes. Equations and validity limits are in
[the plume architecture](../architecture/propulsion-plume.md).

The Desktop Griffin copy referenced new BellNozzle outputs while the
optimization checkout still loaded the preceding library (90 documents).
The complete updated library has 92 documents. Deploy its models, package
order, shader, shared visuals and Twin connections together. Missing physical
state remains an error; no compatibility values or alternate reader were added.

Production validation used optimization's `target/debug/luncosim`, banner
2b8f54d9, with this change's explicit asset root. No Rust rebuild was needed.

- Rocket plume: 48 conservation/optical-budget/transport checks passed in a
  windowed runtime. An additional five invalid-gas checks verified the typed
  Modelica fault, frozen last accepted sample and surviving application.
- Shared RCS feed starvation: 16 checks passed, including each missing reactant
  and restoration.
- Shared lander plume activity: six checks passed at half command. Full stock
  command already exceeds the existing 60 kN actuator admission bound on the
  unchanged baseline; this gate deliberately stays inside that range. This
  change does not qualify stock full-throttle actuation.
- Griffin fuel and oxidizer exhaustion: 11 checks each passed. The fuel test
  exposed the old unbounded O/F pressure feedback; limiting-reactant mass
  balance repairs that owner, including chemical heat accounting.
- Griffin engine-out: 20 checks passed. Activity uses the same 1e-9 extinction
  fraction as the optical tail, because valve closure is exponential.
- Windowed main engine nominal/partial/off, individual engine-out, RCS on/off,
  moving texture and paused phase were checked through ports and captures.
  GPU compilation/validation logs were clean. Captures and logs are retained
  locally under `/tmp/griffin-physical-plume-20261019`.
- The Desktop surface-operations scene completed coupled initialization and
  reached readiness with no runtime fault using the updated assets.
- Skill catalog validation passed (43); Griffin source/evidence verification
  passed (214 targets/evidence records, 55 sources, three roles).

Optical coefficients and colour remain uncalibrated study inputs. This is a
forward supersonic ideal-gas model, not CFD, equilibrium chemistry, spectral
radiometry, rarefied backflow or plume/surface interaction. Image review proves
an operational soft volume, not measured Griffin flame fidelity.

All owned validation sessions use explicit ports and API Exit with port-release
checks. User port 3743 was not controlled. Unrelated main worktree physics and
USD edits are outside this change.
