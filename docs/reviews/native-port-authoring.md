# Native port authoring on projected program bodies

`live_runtime_port_exists` in `lunco-usd-commands` validates native endpoints for
`ApplyUsdOps`. The stage and prim may have several projected entities. Preview
entities are excluded and a matching entity without the requested port cannot
terminate the search before the live native owner is considered. `PortRegistry`
remains the authority for endpoint existence and direction.

Production regression exercised through the document API:

1. Mount a rigid body that also hosts a running Modelica program, with its source
   document preview open.
2. Read native `position_y` through `ReadPorts` (source Avian, type f64).
3. Connect a declared double input to `outputs:position_y_typo`: rejected 409,
   document unchanged.
4. Connect that input to native `outputs:position_y`: accepted, save succeeds,
   Modelica recompiles and native physics is admitted.

The observed measurement was 0.4846 m. The previous lookup rejected the valid
endpoint after finding a preview/proxy first. No synthetic output property or
name-based native endpoint exemption is involved.
