# Rumoca source admission

LunCoSim consumes Rumoca 0.10.2 from the maintained `main` branch.
`Cargo.lock` retains the exact compiler/solver revision
`e3da928cce08b794c6ec78ef85d2db0bd521547f`; locked builds therefore use one
reproducible source snapshot. Dependency updates advance this lock explicitly.
The maintained dependency owns lexical/import-aware namespace dependencies,
base-class lookup context, immutable cached class interfaces, and comprehension
analysis of continuous and initial structured equation templates. Hosts consume
`rumoca_compile::compile::source_root_dependencies` directly and map those facts
to the common library registry. Namespace placement through `within` does not
itself require a library. Import aliases have no host-specific handling.

Dependency discovery is a language mechanism, governed by Modelica's lexical
rules rather than a changeable scenario policy. It reuses Rumoca's interface and
import lookup. Document edits, source-set admission, execution ownership and
history remain with their existing application owners. Component edits feed the
same document/session path as compilation and projection.

Lint jobs capture source generation with source text and reject superseded
completions. Parser channels remain pending until their syntax generation is
current. Scratch closure cancels live work and retains completed history; UI
document cleanup does not delete that history. Desktop and server builds enable
the same artifact service.

Scene and preview admission publish the same root/runtime recipe. The existing
canonical-stage synchronization owner applies disposable view opinions after
admission. This preserves the source capture fence during Summer scene loading.

Verification:

- Native compiler checks: 397 suite checks passed; the remaining source-span
  assertion was updated to the parser's complete-declaration contract and passed
  on its focused rerun. Dependency fixtures cover arbitrary nested aliases,
  quoted/local declarations, import forms and missing external namespaces.
- The fixed 20-model MSL canary completed at the dependency revision above.
  Twelve simulated models were compared with OpenModelica without a deviation;
  eight models were rejected before comparison. This is a focused canary, not
  a full MSL compatibility claim.
- Nineteen production Modelica gates passed, including structural authoring of
  an imported temperature type, Kelvin metadata, source identity, bounds,
  isolation, event samples, result budgets and generation-scoped lint.
- Production history seed and restore passed three and five checks respectively:
  source closure, durable UUID artifact, immutable source CID, changed same-name
  source, and rejection of a malformed optional artifact.
- The browser worker compile check passed. Browser runtime behavior was not
  exercised.
- Summer startup passed six checks. All three live models remained Running
  without Modelica errors and advanced 89.5 simulation seconds over 90.03 wall
  seconds. The production screenshot is `target/summer-school-final-route.png`.

Optional networking command documentation is retained from the integrated
target branch; descriptions for commands present in the captured desktop schema
are regenerated from that schema and current source.
