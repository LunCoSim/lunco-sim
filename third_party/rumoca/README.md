# Rumoca compile provenance, solver lifecycle, and output admission

Source: https://github.com/LunCoSim/rumoca at
`eaa5291ff610085cfc02f9673fcb393245feaa9b`, Apache-2.0 (see LICENSE).
`rumoca-compile`, `rumoca-phase-dae`, `rumoca-solver`, and `rumoca-eval-solve` are vendored; all other Rumoca
packages retain that exact Git revision. Their manifests are standalone and
retain the upstream dependency versions and features.

The patch provides a scoped, thread-local cancellation guard backed by the
admitted native run's atomic flag. Evaluation and simulation-driver boundaries
return errors when cancellation is requested. The runner reports Cancelled and
releases its scheduling slot. Numerical methods, stepping, and output grids
remain owned by the existing solvers. A numerical kernel already executing
returns before the next checkpoint can observe cancellation.

A separate scoped output budget fences retained scalar columns and their time
vector. The maintained timeline iterator supplies an allocation-free requested
grid count; the central visible recorder checks additional event samples and
reserves growth fallibly. Recorded times are appended only after their values
succeed. Captured limits are local to the admitted native thread or Web Worker;
nested guards restore their caller. This does not alter event/sample selection.

Updating the Rumoca pin requires reapplying and testing these owner boundaries
against the new revision.

The compile patch exposes the strict successful DAE boundary's actual target and
participating source URI/source-set keys. It consumes the existing reachable
closure and backing-key query, without walking a second dependency graph or
changing compile selection. Non-strict DAE construction has no such closure.
LunCo's compiler captures portable source-content CIDs from these exact
contributions before evicting user documents; host paths and runtime mount IDs
never enter the persisted identity. Parsed-only participating roots explicitly
lack authoritative byte identity. Unrelated parsed roots do not block artifacts.

The DAE phase retains external input bindings as initialization expressions at
their fully qualified variable paths. Internal and connected inputs retain
equation ownership. Binding conversion omits only actual DAE inputs, preserving
class, instance, inheritance, and parameter-expression scope without rewriting
source. The compiler declaration-scope test covers library admission, repeated
instances, internal bindings, and invalid defaults; production readback is
covered by `modelica_scoped_input_defaults.rhai`.
