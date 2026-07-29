# Models

`LunCo/` is the reusable packaged Modelica library, grouped by physical or
engineering domain. Keep equations and continuous state here; expose scalar
inputs/outputs and acausal connectors for USD assemblies to compose.

Top-level `.mo` files are standalone demonstrations or focused runtime models.
New reusable production models should normally live in
`LunCo/<Domain>/<Class>.mo` with the matching `package.order` entry.

The runtime parser intentionally supports the project subset of Modelica. Avoid
`import`; use a local type name inside the same package and a fully qualified
name such as `LunCo.Electrical.Pin` across packages.

Do not copy assembly topology into a rover-level model. Reference component
models from component USD assets, then collect and connect those parts in the
vehicle's USD `CollectionAPI:components`.
