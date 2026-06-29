# lunco-tools-rhai

The **rhai adapter** for the runtime-agnostic `lunco-tools` registry.

Provides the two concrete `Tool` impls scenarios use today and binds every
registered tool into a rhai engine so it is callable as `name::fn(...)` from
anywhere — including inside pure hook functions like `on_tick`.

## Key API

- **`RhaiTool`** — a tool authored in **rhai source**. Its functions become a
  compiled rhai module, running with full rhai semantics (closures, prelude,
  host verbs) — exactly like the scenario itself.
- **`NativeRhaiTool`** — a tool backed by **native Rust** functions (a builder
  closure registers bridge functions). A tool authored in another runtime
  (Python, …) is exposed to rhai as a `NativeRhaiTool`.
- **`refresh(&mut Engine)`** — binds every registered tool into the engine as a
  **static module** (script-level `import` aliases are invisible to rhai's pure
  hook functions; static modules are not). Returns the `(name, source)` pairs.
- **`register_rhai_tool(name, source)` / `register_native_tool(...)`** —
  convenience registration into the global `lunco-tools` registry.

So `refresh` only ever handles "source-defined" or "native" — every backend
funnels through one of those two paths.
