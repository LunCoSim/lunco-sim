# Rumoca solver lifecycle and output admission patch

Source: https://github.com/LunCoSim/rumoca at
`31d5a831cb2ce53a524354e0319572b0de995538`, Apache-2.0 (see LICENSE).
Only `rumoca-solver` and `rumoca-eval-solve` are vendored; all other Rumoca
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
