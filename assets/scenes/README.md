# Scenes

- `base/` contains reusable world foundations.
- `luncosim/` contains interactive application and demonstration scenes:
  `crater_cup.usda` starts in the avatar overlooking Polar Works. Auger hauls
  icy regolith 43 m to the retort. Mule carries spent material 38 m to the spoil
  bank. Dewar collects gaseous hydrogen and oxygen and travels 48 m to Kestrel.
  After refueling and tanker clearance, Kestrel hops and lands for another cycle.
  Select Kestrel to fly it using the shared lander bindings. With the default
  keymap, Space applies thrust, WASD tilts, and Q/E yaws. **Avatar view**
  releases the controls and hands flight back to pad guidance.
  The avatar camera follows Comet's heading and stays level as Kestrel tilts.
  Equipment labels show live masses or battery charge. The ore and tailings
  meshes use object-space regolith detail to carry their surface with the rover.
  Click a receiver for gas mass, absolute pressure and temperature. Click the
  solar field or battery bank for live power and storage. **Object plots** opens
  the selected equipment’s history. Drive Scout with WASD, aim its camera with
  I/K (tilt) and J/L (pan), or race Comet over the jump. **Avatar view** returns
  to the base. The assay press packs a small side stream of the dry residue.
  The field has 64 tables of 96 m² each. Four 250 kWh cabinets buffer the 800 V
  bus. The fixed lighting rig represents one illuminated operating period.
  The feed assumes 5% recoverable water and the electrolyzer uses 55 kWh per
  kg of hydrogen. These are scenario design points. [LCROSS measurements](https://ntrs.nasa.gov/api/citations/20160011273/downloads/20160011273.pdf)
  and [DOE PEM electrolysis targets](https://www.energy.gov/cmei/fuels/technical-targets-proton-exchange-membrane-electrolysis).
  `regolith_haul.usda` is a separate tipper camera study.
  `sandbox_scene.usda` is the engineering regression yard, `lander_ops.usda` is the
  interactive powered-descent mission, `lander_cinematic.usda` demonstrates
  event-led camera direction, and `solar_rover_demo.usda` demonstrates solar
  power and route following. `selection_intents.usda` is an editor review
  fixture for replacing and extending selected USD targets.
- `tests/` contains focused executable regression scenes.
- `celestial/` contains celestial-specific examples.

A scene composes components, vessels, structures, lighting, and behavior. Do not
inline a reusable physical part merely because one scene currently uses it.

Regression scenes should have one observable purpose, deterministic assertions,
and a `LunCoProgramAPI` test/scenario script where behavior must be assessed.
Cross-domain acceptance belongs in one deliberately integrated scenario; small
regressions remain focused so failures are diagnosable.

The [`vessels` guide](../vessels/README.md) maps the built-in rover and lander
assets. The [`scenarios` guide](../scenarios/README.md) explains the behavior
programs composed by the interactive examples.
