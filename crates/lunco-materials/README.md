# lunco-materials

Self-contained material plugins for LunCoSim's USD-driven rendering pipeline.

## Architecture

Each material is a **fully independent Bevy Plugin** that can be added to any App without cross-crate dependencies or manual configuration:

```rust
app.add_plugins(SolarPanelMaterialPlugin)
   .add_plugins(BlueprintMaterialPlugin);
```

Each plugin handles:
1. **Shader embedding** — WGSL compiled into the binary at compile time via `load_internal_asset!`
2. **Material registration** — `MaterialPlugin<T>::default()` for Bevy's render pipeline
3. **Auto-assignment** — post-sync system runs after `sync_usd_visuals`, reads `primvars:materialType` from USD, and applies the material

## Adding a New Material

### 1. Define your extension

```rust
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone, Copy)]
pub struct MyExtension {
    #[uniform(100)]
    pub my_param: f32,
}

impl MaterialExtension for MyExtension {
    fn fragment_shader() -> ShaderRef {
        MY_SHADER_HANDLE.into()
    }
}
```

### 2. Register your shader

```rust
pub const MY_SHADER_HANDLE: Handle<Shader> = Handle::Uuid(MY_UUID, PhantomData);

pub struct MyMaterialPlugin;

impl Plugin for MyMaterialPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(
            app,
            MY_SHADER_HANDLE,
            "../../../assets/shaders/my_material.wgsl",
            Shader::from_wgsl
        );
        app.add_plugins(MaterialPlugin::<MyMaterial>::default());
        app.add_systems(Update, apply_my_material.after(lunco_usd_bevy::sync_usd_visuals));
    }
}
```

### 3. Create the post-sync system

```rust
pub fn apply_my_material(
    mut commands: Commands,
    stages: Res<Assets<UsdStageAsset>>,
    mut materials: ResMut<Assets<MyMaterial>>,
    q_all: Query<(Entity, &UsdPrimPath), (With<Mesh3d>, Without<MyMaterialApplied>)>,
) {
    for (entity, prim_path) in q_all.iter() {
        let Some(stage) = stages.get(&prim_path.stage_handle) else { continue };
        let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { continue };
        let reader = (*stage.reader).clone();

        let mat_type: Option<String> = reader.prim_attribute_value(&sdf_path, "primvars:materialType");
        if mat_type.as_deref() != Some("my_material") { continue; }

        let mat = create_my_material(&reader, &sdf_path, &mut materials);
        commands.entity(entity).insert((
            MeshMaterial3d(mat),
            MyMaterialApplied,
        ));
    }
}
```

### 4. Define the USD primvars

```usda
def Cube "MyObject"
{
    string primvars:materialType = "my_material"
    float primvars:myParam = 42.0
}
```

### 5. Register in the binary

```rust
app.add_plugins(MyMaterialPlugin);
```

That's it. No changes to `lunco-usd-bevy` or any other crate needed.

## USD Convention

All materials use the **`primvars:`** namespace (USD-standard for per-geometry shader parameters):

| Attribute | Purpose |
|-----------|---------|
| `primvars:materialType` | Material selector (e.g. `"solar_panel"`, `"BlueprintGrid"`) |
| `primvars:cellColor` | Solar panel cell color |
| `primvars:gridMajorSpacing` | Blueprint major grid spacing |
| ... | Each material defines its own primvars |

## Existing Materials

### SolarPanelMaterial
- Procedural photovoltaic cell grid (no textures)
- 11 tunable parameters: cell layout, bus lines, glass reflection, frame border
- Shader: `assets/shaders/solar_panel_extension.wgsl`

### BlueprintMaterial
- Blueprint-style grid pattern for terrain
- 6 USD-configurable parameters + 10 shader defaults
- Shader: `assets/shaders/blueprint_extension.wgsl`
- **Re-exported by `lunco-celestial`** for use by terrain tiles, celestial body spheres, and visual transition systems

## Crate Dependencies

```
lunco-materials
    ├── bevy
    ├── openusd
    └── lunco-usd-bevy          (for USD post-sync systems)

lunco-celestial  →  lunco-materials   (BlueprintMaterial consumer)
```

`lunco-materials` is the **canonical source** for all material types. `lunco-celestial` imports and re-exports them for convenience — downstream code can import from either crate.

## Crate Structure

```
crates/lunco-materials/
├── src/
│   ├── lib.rs              # Re-exports + shared get_attribute_as_vec3() helper
│   ├── solar_panel.rs      # SolarPanelMaterialPlugin + shader + post-sync system
│   └── blueprint.rs        # BlueprintMaterialPlugin + shader + post-sync system
└── tests/
    └── materials_test.rs   # Extension defaults validation
```
