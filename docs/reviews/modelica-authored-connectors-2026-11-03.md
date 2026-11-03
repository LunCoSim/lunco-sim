# Authored Modelica connector graphics

The diagram interface now includes public instances whose resolved class is a
Modelica connector. Plain causal Real declarations remain available to semantic
connections, telemetry and inspectors; they do not imply a graphical terminal.
The shared declaration snapshot owns the classification. No renderer-specific
name lists or film-only hiding rules are used.

The affected LunCo electrical and mechanical classes author standard Placement
annotations for actual terminals. The declarations, parameters and equations
are otherwise unchanged. Short connector definitions and inherited connectors
are retained; protected connectors are excluded from the public interface.

Reference: Modelica 3.6 section 9.1 and section 18.6 component annotations.
https://specification.modelica.org/maint/3.6/connectors-and-connections.html
https://specification.modelica.org/maint/3.6/annotations.html

Validation:
- Focused causal/short/inherited/protected connector regression: passed.
- All 11 diagram tests in lunco-modelica-core: passed.
- Debug luncosim build: passed.
- Skill catalogue: passed, 43 skills.
- Native visible session PID 1947264, API 4493, using target/debug/luncosim:
  full FLIP power network inspected; unauthored tiny causal markers absent.
  Screenshot and executable receipt retained in the Griffin V13 R4 film evidence.

The user explicitly approved continuing locally without Trello because no Trello
connector is available. Changes remain local on optimization; nothing pushed.
