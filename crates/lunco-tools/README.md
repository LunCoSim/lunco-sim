# lunco-tools

Backend-agnostic **tool** registry.

A *tool* is a named, reusable bundle of callable functions — a library of
selection / behaviour policy a scenario can call as `name::fn(...)`. The key
design point: a tool's **implementation is pluggable**. It may be authored in
rhai source, in native Rust, or (later) in any other runtime — all are the same
`Tool` to this crate.

That extensibility is why this crate is deliberately **dependency-free**: it
owns only the *abstraction* + the global registry + discovery. The actual
binding of a tool into a script runtime lives in an adapter crate (e.g.
`lunco-tools-rhai`), so non-rhai consumers can still enumerate and describe
tools without pulling rhai in.

## Key API

- **`Tool`** — trait: a named bundle of callable functions, language-neutral.
  Metadata methods are runtime-neutral (for discovery); a runtime adapter
  downcasts via `Tool::as_any` or reads `Tool::source` to actually bind it.
- **`register(Arc<dyn Tool>)`** + the global registry / discovery functions.

```rust
// a native Rust adapter registers a tool…
lunco_tools::register(Arc::new(MyNativeTool));
// …a rhai adapter (lunco-tools-rhai) binds every registered tool as `name::fn`.
```

## Status

Phase 1 done (trait + registry + discovery). Twin persistence is the next phase.
