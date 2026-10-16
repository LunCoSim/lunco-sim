# Modelica editor package scope

2026-10-16. The Summer Space School editor path now projects a namespaced
Modelica file on fresh open, after scene restart, and after Twin replacement.

`SolarPanel.mo` parses successfully and exposes document-relative candidate
`SolarPanel`. Its source declares `within LunCo.Electrical`. The canvas checked
that bare name against the local AST, which accepts document-relative lookup,
then sent the unchanged name to the shared compiler. The compiler stores the
qualified identity, so projection failed with `Modelica diagram target
SolarPanel is unavailable` even though document parsing had completed.

Canvas projection now obtains the exact compiler identity from Rumoca
`StoredDefinition::qualified_class_name` before requesting its declaration
snapshot. The maintained AST query owns source scope and quoted identifiers.
There is no editor-side prefix parser, guessed alias, readiness override or
reload retry. Missing declarations still produce the owning projection error.
The canonical contract is in `docs/architecture/20-domain-modelica.md`.

Regular `cargo run -j 4 -- --api 4384 --no-vsync --no-throttle` built and launched
the production windowed host. The owned PID, executable and checkout were
verified before API control. Opening the Summer Twin and the maintained
`assets/models/LunCo/Electrical/SolarPanel.mo` reproduced the failed diagram;
`target/solar-panel-initial.png` was captured and inspected before the fix.

After the fix, three completed scene restarts retained the same editable
document within each cycle. Three reopenings of the same Summer Twin retired
the outgoing document and admitted a fresh SolarPanel document. Each restart
waited for fresh generated scene documents and the projected scene, and each
fresh file waited for its completed nonempty canvas projection. All six
operations completed without a SolarPanel projection error. API document/parse
receipts are in `target/solar-panel-reloads.json`; renderer completion records
are in `target/coordinate-drive-cargo-run-final.log`. Captures
`target/solar-panel-restart-3.png` and `target/solar-panel-twin-3.png` show the
retained and reopened diagrams. The wrapper exited 0. Skill catalogue and
`git diff --check` also pass.

This matches a concrete editor failure in the reported presentation workflow.
The original presentation process logs were not supplied, so this evidence does
not identify every state that may have existed in that particular session.
It does not establish Modelica compilation or experiment/physics acceptance
merely from an editor diagram.
