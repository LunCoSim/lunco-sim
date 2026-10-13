# Bevy render source provenance

This is the published `bevy_render` 0.19.1 package from Bevy commit
`b56fc29d3016e641754765244b5ba3f9cc504671`, with its MIT and Apache licences.
The crates.io archive SHA256 is
`fc86a32b2eef9859e1e7d323c6d1a9150fe77aa17c30fd05abebc12f645929dc`.
Registry installation metadata and the package-local generated lockfile are
omitted; the repository lockfile resolves the dependency graph.

`UninitBufferVec` allocates GPU-written output slots using the existing
`ShaderSize::SHADER_SIZE` contract. Host alignment can differ from WGSL:
scalar math makes `MeshUniform` 164 bytes in Rust while its shader stride is
176 bytes. Allocation must include every complete shader record. This changes
neither mesh input serialization nor simulation math, culling, or quality.
The native stride test covers a 20-byte host record with a 32-byte shader stride.
The public constructor and buffer APIs remain unchanged.

The root patch consumes this package directly. Its development profile retains
the upstream dependency's level-3 code generation. Future dependency updates
must retain the shader-stride allocation contract.

Trailing whitespace in upstream shader comments and package documentation is normalized.
