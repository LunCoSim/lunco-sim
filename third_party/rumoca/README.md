# Rumoca compile provenance, solver lifecycle, and output admission

Source: https://github.com/LunCoSim/rumoca at
`31d5a831cb2ce53a524354e0319572b0de995538`, Apache-2.0 (see LICENSE).
Only `rumoca-compile`, `rumoca-solver`, and `rumoca-eval-solve` are vendored; all other Rumoca
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
