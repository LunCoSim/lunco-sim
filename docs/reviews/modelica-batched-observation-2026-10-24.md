# Batched Modelica observation collection

## Current owner and measured cost

`collect_stepper_observables` in `lunco-modelica-worker` calls `LiveStepper::state`,
checks the named observation and converts its native `IndexMap<String, f64>` to
`Vec<(String, f64)>`. Rumoca's `MeSimulationSession::visible_values` clones the
component's output names, reads its existing batched f64 output, constructs the
map and overlays host-applied inputs. The worker then discards that map. This
is distinct from main-thread publication into `ModelicaVariables`.

The public ME host already exposes `output_names` and `output_values`. Native
checked lowering includes scalar inputs in the visible inventory. The public
RK45 wrapper retains the names through `variable_names` but hides the batched
reader behind its private common backend. `values_for` still constructs the
complete named map. Rebuilding the adaptive host in LunCoSim would duplicate
native construction policy, so it is not the proposed solution.

The ignored `named_and_batched_observation_collection` diagnostic in
`lunco-modelica-solver` compares the existing named collection with native
batched collection into shared `Arc<str>` labels. It uses one checked inline
model with 2,304 outputs, an input and a parameter-dependent equation. The
native session constructs once; five communication points change the input and
time. Every pair checks all names and f64 bits, including the current input.
Both timed paths validate finite observations, filter `time`, retain ownership
of the resulting rows and include their destruction. The candidate also checks
name/value cardinality. Label preparation, compilation, construction, input
application and advancement are outside the collection timing.

One regular-profile command, using the normal target and sccache, passes:

```sh
cargo test --locked -p lunco-modelica-solver --lib \
  observation_diagnostics::named_and_batched_observation_collection -j 4 -- \
  --ignored --nocapture
```

Five alternating pairs of 512 reads give median 134.721 ms named versus
38.450 ms batched, approximately 71.46 percent less collection time. Width is
2,305 values. This is a bounded diagnostic, not a worker, complete coupled-tick
or startup gain. It covers the checked native linked component, not arbitrary
imported FMUs. No production observation path changes in this commit.
Raw output remains ignored at
`.cache/codex-evidence/performance-20261023/modelica-observation-collection-diagnostic.log`.

## Proposed SDK extension

Dependency approval is required before changing or publishing Rumoca source.
The proposed change in `../rumoca` / `LunCoSim/rumoca` is limited to exposing the
existing reader through `rumoca-sim`'s common backend and public RK45 session:

```rust
pub fn output_values(&self) -> Result<Vec<f64>, SimError>
```

The common backend forwards to `MeSimulationSession::output_values`; RK45
forwards to that backend. Keep its current constructor, execution admission,
adaptive plugin, lifecycle, accepted-point observation, warnings and error
propagation. No integrator or equation changes, reimplemented input overlay,
global cache, alternative constructor or application copy of private policy.

The application would consume that API for ordinary adaptive sessions and the
same public ME reader for fixed sessions. The session owns immutable shared
labels obtained from its native inventory. All native inputs must have an
observation entry; mismatch rejects construction rather than inventing a value.
Each read checks the exact width, finite clock and complete finite sample before
publication. Cut over worker transport, accepted-step samples and consumers
together to shared labels, preserving the external serialized shape and
current input visibility. Initial input capture remains a checked native read.
Reset/replacement retires the labels with its exact session; no retained batch
may cross session admission. Existing input/parameter classification stays live.

Required verification: native named/batched equivalence for current inputs,
aliases, arrays, changed inputs, events, reset and lifecycle rejection; the
twenty-model MSL canary; complete application collection/publication comparison;
production variable projection, missing-state and observation-failure negatives,
metadata and exact accepted-step sample gates. Then inspect a normal landed
Griffin window and a separate qualified Tracy capture. No performance threshold
is satisfied by the collection diagnostic alone.
