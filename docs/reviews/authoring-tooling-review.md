# Unified authoring tooling review

This review records the generic editing architecture for any LunCoSim Twin.
It is deliberately independent of a particular vehicle or mission.

## Decision

Use one document/edit substrate and thin format adapters:

```text
Editor / API / Rhai policy
          |
    dynamic Rhai tools
          |
  typed command/query adapters
          |
lunco-doc Document + DocumentHost + DocumentRegistry
          |
format owner (USD / Modelica / SysML / Rhai / shader)
```

`lunco-doc` supplies identity, generation, typed reversible operations,
atomic grouped apply, optimistic parent checks, and the undo/redo contract.
`lunco-doc-bevy` adds per-domain registries, lifecycle events and journal
recording. Generic document verbs (`UndoDocument`, `RedoDocument`,
`SaveDocument`, `SaveAsDocument`, `CloseDocument`, `ForkDocument`) stay
format-neutral. A format adapter owns only parsing, lowering, diagnostics and
projection.

The authored interaction surface is Rhai. Tool files under
`assets/scripting/tools/` are dynamically discovered and hot-reloadable, so a
Twin can add policy and UX without rebuilding Rust. The current reusable
facades are:

- `assembly_edit` for typed USD operations, proposals, Editor previews and
  projection-aware inspection;
- `modelica_editor` for generation-checked AST/diagram/text batches and
  compile checkpoints;
- `sysml_editor` for source-range/whole-source edits and semantic checks; and
- `rhai_editor` for source-range/whole-source script edits and compile checks;
- `authoring_session` for domain capability discovery, dry planning, grouped
  apply, checkpoint, undo and redo.

## Editor interaction contract

The expected experience is the same for every format:

1. Discover the exact document and current generation (and USD edit target or
   selection when relevant).
2. Build a pure Rhai plan. Show affected paths/symbols, operation count,
   source revision and any diagnostics; do not mutate during planning.
3. Commit one reviewed intent as one grouped operation. The owner validates
   every operation before changing state and rejects stale or read-only edits.
4. Wait for the matching owner projection. Read back typed facts and display
   diagnostics; for USD, preserve the active camera and selection while the
   composed stage refreshes.
5. Offer one-document undo/redo and an explicit save/checkpoint. A failed
   projection or compile is visible and retryable; it is never replaced by a
   guessed active tab, alternate writer or silent default.

This is the same incremental loop used by established CAD/CAE/DCC systems:
component-local frames and histories, named selections/relationships, a
reviewable feature sequence, and an assembly-level integration check. OpenUSD
EditTargets and composition opinions are respected; Modelica source, AST and
diagram annotations stay linked; SysML source remains the normative requirement
and traceability artifact.

## Efficiency rules

- Batch only the operations belonging to one user intent; grouped apply parses
  or projects once and creates one undo entry.
- Keep reads incremental: use generation/source-revision cursors and typed
  queries instead of copying a complete stage or source on every tick.
- Let Modelica structural edits defer to its asynchronous parse gate rather
  than blocking the Editor thread.
- Keep USD preview transforms transient and session-scoped; do not author a
  camera/explode pose into the source unless it is an actual design fact.
- Keep canonical quantities as native `f64`/USD `double` and make renderer-only
  narrowing explicit.
- Run one production session at a time. A second simulator or build process
  competing for the same target invalidates timing and makes interactive
  feedback misleading.

## Remaining bounded gaps

The substrate is present, but the following are still useful follow-on work:

- a format-neutral `InspectDocument` query that aggregates adapter snapshots
  without exposing a second registry;
- deferred command acknowledgements for adapters whose apply must wait for an
  asynchronous projection/parse, so the Editor can correlate completion
  without polling; and
- a shared diagnostics panel that renders source spans and affected identities
  for every adapter while retaining domain-specific details; and
- a `ShaderDocument` adapter. WGSL is currently asset/renderer-owned through
  `CreateShader`/`ImportShader`, so source-range shader edits should not be
  exposed until they use this same identity, generation and history contract.

These are infrastructure improvements. They should be implemented once in
the generic document/workbench layers, not re-created in Twin scripts.

## Standards and practice references

- [OpenUSD EditTarget](https://openusd.org/dev/api/class_usd_edit_target.html)
  defines where authored opinions are written in a composition.
- [OpenUSD namespace editing](https://openusd.org/25.11/user_guides/namespace_editing.html)
  describes preserving dependent paths when moving/removing scene structure.
- [Modelica Language Specification](https://modelica.org/documents/MLS.pdf)
  defines textual and graphical/annotation representations of a model.
- [SysML v2 specification](https://www.omg.org/spec/SysML/2.0/Language/PDF)
  defines requirements, structure, behavior, analysis and verification as
  related model concerns rather than independent checklists.
- [Blender transform introduction](https://docs.blender.org/manual/en/latest/scene_layout/object/editing/transform/introduction.html),
  [FreeCAD Part Design](https://github.com/FreeCAD/FreeCAD-documentation/blob/main/wiki/PartDesign_Workbench.md),
  [Fusion components](https://help.autodesk.com/view/fusion360/ENU/?contextId=ASM-COMPONENTS),
  [SOLIDWORKS mate practices](https://help.solidworks.com/2024/English/SolidWorks/sldworks/c_Best_Practices_for_Mates_SWassy.htm),
  and [COMSOL named selections](https://doc.comsol.com/6.3/doc/com.comsol.help.comsol/comsol_ref_visualizationselection.22.25.html)
  all reinforce local datums, stable identities, incremental feature histories
  and named association boundaries.
