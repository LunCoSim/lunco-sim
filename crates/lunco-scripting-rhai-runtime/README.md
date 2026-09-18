# lunco-scripting-rhai-runtime

Production Rhai runtime integration for LunCoSim.

This package owns the application-facing interpreter boundary:

- Rhai commands;
- tool and timeline persistence;
- `.rhai` asset loading and import dependency tracking;
- composition of the language-neutral host with
  [`lunco-scripting-rhai-world`](../lunco-scripting-rhai-world).

[`lunco-scripting`](../lunco-scripting) remains the language-neutral host for
documents, backend-neutral scenario lifecycle, and optional Python support.
[`lunco-scripting-rhai-world`](../lunco-scripting-rhai-world) owns the reflected
world bridge, `RhaiScenarioRuntime`, authored policy activation, and optional
Twin-scoped native hook providers.
[`lunco-scripting-rhai-core`](../lunco-scripting-rhai-core) remains the
reusable interpreter substrate, while [`lunco-scripting-rhai`](../lunco-scripting-rhai)
owns catalog, diagnostics, and dataset-query projections.

Install `LunCoScriptingRhaiRuntimePlugin` in an application. It installs the
language-neutral scripting plugin when necessary and registers the Rhai
runtime's reflected commands and lifecycle systems.
