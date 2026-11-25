# Live simulation and authored USD verification

The [runtime and authoring contract](../architecture/50-usd-driven-visuals.md#live-values-and-saved-settings) describes the commands for operating a mounted Twin, changing a composed preview and committing an edit. Use document generation and projection events together with the physical or visual result to verify each interaction.

## Source and acceptance coverage

| Source | Production evidence and coverage |
|---|---|
| Factory site | On the Factory site from `b2fbc79` (interaction migration committed as `0ec42fa`), the loading-facade door changed collision state and 100 billboard writes kept the document generation and both entity IDs stable. No projection event occurred during those live writes. Six window and wall edits projected in place, and introducing a billboard projected its subtree. Startup, the window material edit and the closed-door capture each had zero magenta pixels in the sampled 640 × 360 frame. |
| Factory process planner | The production planner framed a station and completed physical delivery at simulation tick 7934. Five packing checks passed, including tote retirement, preassigned load activation, mass conservation and zero document projections during packing and delivery. Creating and retiring the trial geometry are explicit setup and teardown. |
| Space School | All 38 workshop checks passed. Opening and closing the hull, deploying and retracting the screen, and losing and recovering the radio link preserved the stage and document generations. The headless run emitted only the initial projection. |
| Packaged routes, Griffin and FLIP | The route editor passed 126 checks, including an unchanged ribbon geometry revision after a point color edit. The standalone marker passed 12. The native Griffin run passed 11 descent checks, 13 landed checks and three headless checks. The V14 inset uses the capture HUD owner. The full film was outside this run. |
| Roadster films | The real skill-course director hid and restored the crest markers with stable entity and document identities. The Build demonstration retains its explicit journaled placement. Camera calibration and articulated initial conditions are take preparation. The full film was outside this run. |
| Mission examples | With two open documents sharing a prim path, the host-scoped checkpoint check rejected a foreign host. The editor identity fixture passed 13 checks. |
| Engine | Verified live and persistent lighting and undo, composed sublayer ownership, pose and joint commands, shader assignment, visual completion and recovery. Eight texture-specific checks exercised declared textures, pending or failed bindings and type validation. The fixed-Sun test also checked five rendered frames. |

## Inventory and verification

Reviewed durable, locally discoverable source manifests in the simulator assets and the model, scene, and campaign trees under Documents. Checked the desktop Twin recents for additional source roots. Generated run directories, recordings, archive downloads and duplicate simulator source checkouts are outside the source count. The two Factory worktrees and the two solar campaign checkouts are counted separately. Script inspection also covered shared authored behavior outside Twin directories.

| Source family | Manifest paths | Coverage |
|---|---:|---|
| Simulator | 18 | Built-in LunCoSim and classroom plus 16 integration fixtures. Shared routes highlight the active waypoint through its live material view. Crater Cup cargo text uses the live billboard owner. Some fixture manifests deliberately contain invalid inputs. |
| Models | 13 | Factory and its planning worktree, Habitat Marte, Luna2, Griffin, FLIP visual review, Moonbase, HAB1, Moon Base Zero, Space School, its separate workshop, Tesla Roadster Runner and the test Twin. No additional simulation-time stage writer identified in this set beyond the listed cases and shared behaviors. Tesla's explicit restart is an operator action. |
| Campaigns | 6 | Solar Energy Short in two checkouts, Electric Motor Startup, Lunar Rover Shadow Budget, south-pole crater illustration and moon overview. No additional simulation-time stage writer identified in their manifest-owned Rhai. The adjacent Roadster film scripts are listed above. |
| Local scenes | 1 | Sunfall Run. Gameplay uses a live shader. The README and cannon asset describe that control. |

The production `ValidateTwin` namespace check ran with collision policy `error` for all 38 paths. **31 passed, six returned reports with errors and one failed during preparation.** This check inspects sources and namespaces. It does not exercise each Twin's runtime controls.

| Non-passing source | Observed result |
|---|---|
| Factory | Indexed source count exceeded the shared closure preparation budget. The full namespace report is unavailable. |
| Factory planning worktree | Its local branch at `29977d7` predates the live packing and station controls. Incorporate Factory main before using it to qualify interactions. Its namespace check also found two files declaring `FactoryWorkflow` in the same Modelica source root. |
| Space School workshop | 97 errors, including unresolved `lunco://environment/lunar_surface.usda` in the versioned race scene. The separate Space School teaching Twin passed the namespace check. |
| Sunfall Run | Two errors. Its Cybertruck variant uses a path as a prim child name. |
| Three engine fixtures | `dynamic_reference_composition` and `initial_usd_composition_default` report composition dependency errors. `twin_search_paths` reports a tool-name collision. |

## Remaining projection work

- On a graphics host, a route with at least two points and no composed ribbon creates one disposable `RouteRibbon` subtree. Supply the shared ribbon reference in the route source when a graphics workflow requires zero post-mount setup. Subsequent progress and color changes use live presentation.
- An explicit wheel, suspension, tire or attachment edit still refreshes every admitted wheel on the affected stage in place. The wheel owner needs an index of both previous and current attachment targets before it can safely narrow relationship retargets to individual wheels. The Factory live door and label controls do not invoke this owner.
