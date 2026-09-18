# lunco-scripting-rhai-runtime

Production Rhai runtime integration for LunCoSim.

This package owns the high-churn interpreter/application boundary:

- the reflected world bridge and `RhaiScenarioRuntime`;
- Rhai commands, authored policy activation, tools, and timelines;
- `.rhai` asset loading and import dependency tracking;
- Twin-scoped native hook-provider lifecycle.

[`lunco-scripting`](../lunco-scripting) remains the language-neutral host for
documents, backend-neutral scenario lifecycle, and optional Python support.
[`lunco-scripting-rhai-core`](../lunco-scripting-rhai-core) remains the
reusable interpreter substrate, while [`lunco-scripting-rhai`](../lunco-scripting-rhai)
owns catalog, diagnostics, and dataset-query projections.

Install `LunCoScriptingRhaiRuntimePlugin` in an application. It installs the
language-neutral scripting plugin when necessary and registers the Rhai
runtime's reflected commands and lifecycle systems.
