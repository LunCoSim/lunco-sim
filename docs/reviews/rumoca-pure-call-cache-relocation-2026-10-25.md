# Rumoca pure-call result storage relocation

Status: concrete dependency-patch proposal; no dependency source changed.

## Observed owner fault

`rumoca-exec-cranelift::emit::typed_program::input_results::InputResults::enter`
emits the instance-owned `Box<[Cell<u64>]>` address as an integer constant in
every typed pure-call function. `TableCompiler::compile_program` invokes it for
primal and issued directional bodies. Cranelift's native cache key hashes the
complete function stencil, including that constant. A new allocation therefore
changes code identity even when the typed program and all codegen flags match.

The checked Griffin initializer cache record has 34 pure-call owners. Production
warm construction repeatedly reports 16,414 hits and 68 compilations. Those
counts are consistent with recompiling the primal/directional family, but a
native two-live-table regression must establish the exact attribution before
claiming the 68 misses are all removed. No allocator-address normalization or
weaker cache key is proposed.

## Proposed native change

Use Cranelift's existing module-owned writable data objects for pure-call result
cells. Allocate zero-initialized storage through `DataDescription`/`define_data`,
and emit its symbolic address through `declare_data_in_func` and `global_value`.
The maintained JIT relocation path binds the current module's data when cached
code is defined. Keep far-address materialization valid on x86-64 and AArch64;
the data symbol must not assume code/data arenas are colocated.

This replaces literal process addresses and heap-cell retention with native
module ownership. Each table and each primal/directional body still owns fresh
mutable cells. Preserve the table's existing non-Sync contract explicitly if
removing `Cell` storage would otherwise weaken it. The native module remains the
last owner dropped, after all callable addresses become unreachable. Keep the
two-pointer function ABI, complete input-bit comparisons, publish-after-success,
acyclic call rules, numerical checks and independent execution state unchanged.
No application shim, persistent data state, alternate execution path or cache
key relaxation is involved. This is native execution/relocation ownership, not
changeable policy, so no hook is appropriate.

## Required proof and qualification

Add a focused native regression that holds two independently compiled tables
alive under one immutable artifact cache. First demonstrate recompilation on the
unchanged code, then require full warm reuse and interleaved independent outputs
with changed input coordinates. Include primal and directional forms, exact
signed-zero/finite bits, same-input reuse, changed seeds, and owner/module drop
ordering. Changed program bodies must still miss. Retain all existing input
reuse and native artifact relocation checks, and verify the supported far-data
address lowering for both host architectures where tooling permits.

Run the affected owner suites and scoped Clippy, plus the unchanged fixed
20-model Tier 1 MSL/OpenModelica canary with every refusal/missing comparison
visible. Publish only after these gates. Then pin the exact tested Rumoca commit
and run production initialization/receipt, negative faults, lifecycle and exact
replay gates. Measure Griffin first fill separately from a warm restart, retain
all source hashes, inspect landing, and use an owned native Tracy capture for
attribution. Concurrent applications/builds qualify timings; no product speedup
or original timing-target completion is claimed from a helper diagnostic.

## Current baseline and application profiling boundary

No dependency source is modified. The six existing `input_reuse` checks pass on
native revision `45d66677`, covering exact input bits, directional seeds,
independent tables, tensors/assertion predicates, nested calls and failed
execution. The same built owner binary passes all five `native_code_cache`
checks, including function/host-address rebinding and changed code. The explicit
scope-unwind panic is caught by its positive ownership test. These checks do not
assert pure-call artifact reuse; that is the specific missing regression above.

Commands use the production consumer's Rust 1.97.1, regular target and sccache,
with four build/test/Rayon workers. The cache group reuses the already-built
binary. Raw baseline logs stay ignored under
`.cache/codex-evidence/performance-20261023/rumoca-pure-call-relocation-*.log`.

A new binary-cache Tracy build is queued in the application checkout, but
another build owns its target lock and the separate mobility source changes
before compilation. The owned queued Cargo process is identified through its
exact log descriptor, cwd and arguments, then interrupted (exit 130). No app or
collector is launched and no new profile is claimed. Foreign builds, sessions
and source changes remain untouched.

## Approval boundary

The application `AGENTS.md` says: “Ask the user explicitly before vendoring,
forking, or modifying third-party dependency source, or publishing dependency
patches.” This pure-call storage/relocation change is outside the prior immutable
assignment-preparation and mutable sweep-workspace approvals. Requested scope is
the existing `../rumoca` / `LunCoSim/rumoca` fork: implement, test and push this
native fix, then qualify its application pin.
