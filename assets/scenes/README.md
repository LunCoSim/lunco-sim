# Scenes

- `base/` contains reusable world foundations.
- `luncosim/` contains interactive application and demonstration scenes.
- `tests/` contains focused executable regression scenes.
- `celestial/` contains celestial-specific examples.

A scene composes components, vessels, structures, lighting, and behavior. Do not
inline a reusable physical part merely because one scene currently uses it.

Regression scenes should have one observable purpose, deterministic assertions,
and a `LunCoProgramAPI` test/scenario script where behavior must be assessed.
Cross-domain acceptance belongs in one deliberately integrated scenario; small
regressions remain focused so failures are diagnosable.
