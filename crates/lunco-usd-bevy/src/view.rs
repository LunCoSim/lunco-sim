//! `StageView` — composed reads over a **live** openusd `Stage` (Ph0′ substrate).
//!
//! This is the ONE composed-read source: typed reads straight against the live
//! (`!Send`) `Stage`, which every domain extractor reads through.
//!
//! Reads are default-time composed opinions (LIVRPS): references, sublayers,
//! variants, and inherits are resolved by the stage. (Time-sampled / animation
//! reads live with the animation projector, not here.)

use openusd::sdf::{Path as SdfPath, Value};
use openusd::usd::{Collection, PrimPredicate, Stage, compute_included_paths};

/// A borrow of a live composed [`Stage`] offering [`UsdDataExt`]-equivalent typed
/// reads. `!Send` — construct per-system from a `NonSend` `CanonicalStage`.
///
/// [`UsdDataExt`]: crate::usd_data::UsdDataExt
pub struct StageView<'a> {
    stage: &'a Stage,
    /// Precomputed binary (glTF) arc sites, so `resolved_asset` can synthesize
    /// `lunco:resolvedAsset` off the LIVE stage without re-walking the arcs.
    /// `None` for a bare `StageView::new` (tests / non-canonical reads).
    binary_sites: Option<&'a crate::compose::BinarySites>,
}

impl<'a> StageView<'a> {
    pub fn new(stage: &'a Stage) -> Self {
        Self {
            stage,
            binary_sites: None,
        }
    }

    /// Construct with the stage's precomputed binary-arc sites (from
    /// [`CanonicalStage`](crate::CanonicalStage)), enabling `resolved_asset`.
    pub(crate) fn with_binary_sites(
        stage: &'a Stage,
        sites: &'a crate::compose::BinarySites,
    ) -> Self {
        Self {
            stage,
            binary_sites: Some(sites),
        }
    }

    /// The underlying stage (escape hatch for reads not yet wrapped).
    pub fn stage(&self) -> &Stage {
        self.stage
    }

    /// The precomputed binary-arc sites, if this view carries them (used by the
    /// `UsdRead::resolved_asset` synth).
    pub(crate) fn binary_sites(&self) -> Option<&crate::compose::BinarySites> {
        self.binary_sites
    }

    /// A prim's composed `typeName` (e.g. `"Xform"`, `"Mesh"`), if any.
    /// Mirrors [`UsdDataExt::prim_type_name`](crate::usd_data::UsdDataExt::prim_type_name).
    pub fn prim_type_name(&self, prim: &SdfPath) -> Option<String> {
        self.stage
            .prim(prim.clone())
            .type_name()
            .ok()
            .flatten()
            .map(|t| t.to_string())
    }

    /// The default-time composed value of attribute `name` on `prim`, typed as
    /// `T`. Mirrors
    /// [`UsdDataExt::prim_attribute_value`](crate::usd_data::UsdDataExt::prim_attribute_value).
    pub fn value<T>(&self, prim: &SdfPath, name: &str) -> Option<T>
    where
        T: TryFrom<Value>,
        T::Error: std::error::Error + Send + Sync + 'static,
    {
        self.stage
            .prim(prim.clone())
            .attribute(name)
            .get::<T>()
            .ok()
            .flatten()
    }

    /// Attribute `name` on `prim` coerced to a string — handles `String`,
    /// `Token`, and `AssetPath` (the `@…@` form). Inherent helper for the reads
    /// whose value type is genuinely either (`lunco:resolvedAsset`, authored by
    /// the composer as a path but read as plain text).
    pub fn value_str(&self, prim: &SdfPath, name: &str) -> Option<String> {
        match self
            .stage
            .prim(prim.clone())
            .attribute(name)
            .get::<Value>()
            .ok()
            .flatten()?
        {
            Value::String(s) => Some(s),
            Value::Token(t) => Some(t.to_string()),
            Value::AssetPath(a) => Some(a.as_str().to_string()),
            _ => None,
        }
    }

    /// Composed, path-translated targets of relationship `name` on `prim` (the
    /// PCP-resolved targets the flattened reader stored under `targetPaths`).
    pub fn rel_targets(&self, prim: &SdfPath, name: &str) -> Vec<SdfPath> {
        self.stage
            .prim(prim.clone())
            .relationship(name)
            .targets()
            .unwrap_or_default()
    }

    /// Composed members of a standard USD collection.
    ///
    /// `explicitOnly` can use the relationship targets directly. Every other
    /// expansion rule goes through OpenUSD so excludes, subtree expansion,
    /// references, and variants retain standard semantics.
    pub fn collection_members(
        &self,
        prim: &SdfPath,
        instance_name: &str,
    ) -> Result<Vec<SdfPath>, String> {
        let expansion = format!("collection:{instance_name}:expansionRule");
        let includes = format!("collection:{instance_name}:includes");
        if self.value_str(prim, &expansion).as_deref() == Some("explicitOnly") {
            return Ok(self.rel_targets(prim, &includes));
        }

        let collection = Collection::new(prim.clone(), instance_name);
        let query = collection
            .compute_membership_query(self.stage)
            .map_err(|error| format!("could not compute collection membership: {error}"))?;
        compute_included_paths(self.stage, &query, PrimPredicate::DEFAULT)
            .map_err(|error| format!("could not expand collection membership: {error}"))
    }

    /// Attribute `name` on `prim` as a 3-vector (`double3`/`float3`). Mirrors the
    /// legacy `get_attribute_as_vec3` free helper.
    pub fn value_vec3(&self, prim: &SdfPath, name: &str) -> Option<[f64; 3]> {
        self.value::<[f64; 3]>(prim, name)
    }
}

// `rel_target`, `has_api_schema`, `prim_paths` and `children` live on
// [`UsdRead`] ONLY — never add an inherent method of the same name. A Rust
// inherent method silently shadows the trait method, so a duplicate pair can
// differ in return type and each call site picks whichever is in scope. One
// name, one definition.

#[cfg(all(test, not(target_arch = "wasm32")))]
mod compose_tests {
    //! Live-stage composition reads through `StageView` (the openusd composed
    //! Stage): the cross-file inherit/subLayer opinion the entity translator
    //! consumes must land on the vessel after full PCP composition.

    use super::StageView;
    use crate::UsdRead;
    use crate::compose::compose_file_to_stage;
    use openusd::sdf::Path as SdfPath;

    fn asset(rel: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets")
            .join(rel)
    }

    /// Spot-check the composed opinion the live reader must resolve (the
    /// cross-file inherit landing `lunco:port = throttle` on the vessel).
    #[test]
    fn stageview_reads_composed_inherit_opinion() {
        let stage =
            compose_file_to_stage(&asset("vessels/rovers/skid_rover.usda")).expect("compose stage");
        let view = StageView::new(&stage);
        let fwd = SdfPath::new("/SkidRover/Controls/forward").unwrap();
        assert_eq!(
            view.value_str(&fwd, "lunco:port").as_deref(),
            Some("throttle"),
            "StageView must read the cross-file inherited composed opinion"
        );
    }

    #[test]
    fn collection_members_uses_standard_subtree_expansion() {
        let dir = tempfile::tempdir().expect("scratch dir");
        let scene = dir.path().join("collection.usda");
        std::fs::write(
            &scene,
            "#usda 1.0\n\
             def Xform \"World\"\n\
             {\n\
                 def Scope \"Group\"\n\
                 {\n\
                     def Scope \"Child\" {}\n\
                 }\n\
                 def Scope \"Network\" (\n\
                     prepend apiSchemas = [\"CollectionAPI:components\"]\n\
                 )\n\
                 {\n\
                     uniform token collection:components:expansionRule = \"expandPrims\"\n\
                     prepend rel collection:components:includes = [</World/Group>]\n\
                 }\n\
             }\n",
        )
        .expect("write collection scene");

        let stage = compose_file_to_stage(&scene).expect("compose collection scene");
        let view = StageView::new(&stage);
        let members = view
            .collection_members(&SdfPath::new("/World/Network").unwrap(), "components")
            .expect("compute collection members");
        let members: Vec<String> = members.into_iter().map(|path| path.to_string()).collect();

        assert!(members.contains(&"/World/Group".to_string()));
        assert!(members.contains(&"/World/Group/Child".to_string()));
    }

    /// A local `over` must win over the prim it overrides through a `references` arc.
    ///
    /// This is the exact composition shape the video campaign's episode scenes use:
    /// an episode scene `references` a base scene and then re-authors the look of one
    /// of its prims with a sibling `over`, so the base stays usable on its own (it
    /// doubles as the interactive driving tutorial) while the episode gets film
    /// lighting out of the same source.
    ///
    /// Asserted because it silently was NOT happening. MEASURED: episode 01 rendered
    /// its ground at the BASE scene's albedo — mean luma 135/255, a near-white slab —
    /// while its `over "Ground"` authored `(0.13, 0.125, 0.12)` plus a
    /// `material:binding` to the regolith shader. Both opinions were dropped. Episode
    /// 02, which authors the identical look as a plain `def` in its own scene,
    /// rendered at luma 63/255 under an identical sun and exposure.
    ///
    /// An inert `over` is the worst failure mode available here: the scene reads as
    /// though the look were authored, review passes, and the render quietly uses the
    /// base.
    #[test]
    fn local_over_wins_over_a_referenced_prims_opinion() {
        let dir = std::env::temp_dir().join("lunco_over_ref_test");
        std::fs::create_dir_all(&dir).expect("scratch dir");

        // The base: the shape of `assets/scenes/luncosim/lander_ops.usda`.
        std::fs::write(
            dir.join("base.usda"),
            "#usda 1.0\n\
             def Xform \"Base\"\n\
             {\n\
                 def Cube \"Ground\"\n\
                 {\n\
                     float lunco:test:albedo = 0.35\n\
                 }\n\
             }\n",
        )
        .expect("write base");

        // The episode: reference the base, then override that ground's look.
        std::fs::write(
            dir.join("episode.usda"),
            "#usda 1.0\n\
             def Xform \"Episode\" (\n\
                 prepend references = @./base.usda@</Base>\n\
             )\n\
             {\n\
                 over \"Ground\"\n\
                 {\n\
                     float lunco:test:albedo = 0.13\n\
                 }\n\
             }\n",
        )
        .expect("write episode");

        let stage = compose_file_to_stage(&dir.join("episode.usda")).expect("compose episode");
        let view = StageView::new(&stage);
        let ground = SdfPath::new("/Episode/Ground").unwrap();

        // First: does the referenced child exist at all under the referencing prim?
        // If this fails the `over` is moot — the reference arc itself never brought
        // the child across, which is a different (larger) bug.
        let children = view.children(&SdfPath::new("/Episode").unwrap());
        assert!(
            children.iter().any(|c| c.as_str().ends_with("/Ground")),
            "the `references` arc must bring the base's `Ground` child across; got {children:?}"
        );

        let albedo = view
            .value::<f32>(&ground, "lunco:test:albedo")
            .expect("composed ground must carry the attribute at all");

        assert!(
            (albedo - 0.13).abs() < 1e-4,
            "the local `over` must win over the referenced base opinion, got {albedo} \
             (0.35 means the `over` was dropped and the base opinion survived)"
        );
    }

    // ── `collection:*:includes` is a LIST OP, across layers ──────────────────
    //
    // The three tests below take the same collection apart one composition arc at
    // a time. They exist because every solar rover in the repo is silently dead:
    // `project_domain_islands` rejects the electrical network with
    //
    //     output source component `…/SolarPanel` is outside collection `…/Electrical`
    //
    // on `scenes/tests/solar_domain_nested_ref.usda` AND on the shipped
    // `scenes/luncosim/solar_rover_demo.usda`, which has one reference arc and
    // authors the membership in the scene itself. The panel prim composes, the
    // connection to it composes, and the collection that must contain it does not.
    //
    // `collection_members` short-circuits `explicitOnly` to `rel_targets`, i.e. to
    // the relationship's composed targets. Whether a `prepend` in a stronger layer
    // MERGES with a weaker layer's list, or replaces it, or is dropped, is the
    // whole question — and it is a property of relationship list-op composition,
    // not of any asset. The rover assets are only the messenger.
    //
    // Ordered narrowest-first so a run says which arc is the broken one.

    /// One layer, one opinion. The control.
    ///
    /// If this fails, `explicitOnly` membership is broken outright and nothing
    /// below it means anything — there is no composition involved here at all.
    #[test]
    fn explicit_collection_reads_its_own_targets() {
        let dir = tempfile::tempdir().expect("scratch dir");
        let scene = dir.path().join("flat.usda");
        std::fs::write(
            &scene,
            "#usda 1.0\n\
             (\n\
                 defaultPrim = \"World\"\n\
             )\n\
             def Xform \"World\"\n\
             {\n\
                 def Scope \"PanelA\" {}\n\
                 def Scope \"PanelB\" {}\n\
                 def Scope \"Network\" (\n\
                     prepend apiSchemas = [\"CollectionAPI:components\"]\n\
                 )\n\
                 {\n\
                     uniform token collection:components:expansionRule = \"explicitOnly\"\n\
                     prepend rel collection:components:includes = [\n\
                         </World/PanelA>,\n\
                         </World/PanelB>,\n\
                     ]\n\
                 }\n\
             }\n",
        )
        .expect("write flat scene");

        let members = members_of(&scene, "/World/Network");
        assert!(
            members.contains(&"/World/PanelA".to_string())
                && members.contains(&"/World/PanelB".to_string()),
            "an explicitOnly collection must report both of its own authored targets; got {members:?}"
        );
    }

    /// A `prepend` in a STRONGER layer must MERGE with the referenced base's list.
    ///
    /// This is the exact shape `scenes/luncosim/solar_rover_demo.usda` uses: the
    /// rover asset's `power` variant declares the bus members, and the SCENE adds
    /// the panel it mounted with
    ///
    ///     over "Electrical" {
    ///         prepend rel collection:components:includes = [</…/SolarPanel>]
    ///     }
    ///
    /// A `prepend` that REPLACES instead of merging loses the six motors and the
    /// battery; a `prepend` that is DROPPED loses the panel — which is what the
    /// runtime error reports. Both are asserted, so the failure message says which
    /// happened rather than only that the set was wrong.
    #[test]
    fn stronger_layer_prepend_merges_into_referenced_collection() {
        let dir = tempfile::tempdir().expect("scratch dir");
        std::fs::write(
            dir.path().join("base.usda"),
            "#usda 1.0\n\
             (\n\
                 defaultPrim = \"Base\"\n\
             )\n\
             def Xform \"Base\"\n\
             {\n\
                 def Scope \"Bus\" {}\n\
                 def Scope \"Network\" (\n\
                     prepend apiSchemas = [\"CollectionAPI:components\"]\n\
                 )\n\
                 {\n\
                     uniform token collection:components:expansionRule = \"explicitOnly\"\n\
                     prepend rel collection:components:includes = [</Base/Bus>]\n\
                 }\n\
             }\n",
        )
        .expect("write base");

        let scene = dir.path().join("scene.usda");
        std::fs::write(
            &scene,
            "#usda 1.0\n\
             (\n\
                 defaultPrim = \"World\"\n\
             )\n\
             def Xform \"World\"\n\
             {\n\
                 def Xform \"Rig\" (\n\
                     prepend references = @./base.usda@</Base>\n\
                 )\n\
                 {\n\
                     def Scope \"Panel\" {}\n\
                     over \"Network\"\n\
                     {\n\
                         prepend rel collection:components:includes = [</World/Rig/Panel>]\n\
                     }\n\
                 }\n\
             }\n",
        )
        .expect("write scene");

        let members = members_of(&scene, "/World/Rig/Network");
        assert!(
            members.contains(&"/World/Rig/Panel".to_string()),
            "the scene's `prepend` was DROPPED — the member it adds is missing, which is \
             the `outside collection` runtime rejection every solar rover hits; got {members:?}"
        );
        assert!(
            members.contains(&"/World/Rig/Bus".to_string()),
            "the scene's `prepend` REPLACED the referenced base's list instead of merging \
             with it — the base's own members are gone; got {members:?}"
        );
    }

    /// Two variantSets on one prim, each contributing to the same collection.
    ///
    /// `vessels/rovers/rocker_bogie.usda` verbatim: `power = "battery"` DEFINES
    /// `Electrical` with the bus members, and `generation = "solar"` OVERS it to
    /// add the panel. Two sibling variant arcs on the same prim, both authoring
    /// the same relationship — and the selection is made a further arc away, in a
    /// profile asset or a scene, so this is the composition that has to hold for
    /// any purchasable rover configuration to work.
    #[test]
    fn sibling_variant_prepends_merge_into_one_collection() {
        let dir = tempfile::tempdir().expect("scratch dir");
        std::fs::write(
            dir.path().join("rover.usda"),
            "#usda 1.0\n\
             (\n\
                 defaultPrim = \"Rover\"\n\
             )\n\
             def Xform \"Rover\" (\n\
                 prepend variantSets = [\"power\", \"generation\"]\n\
                 variants = {\n\
                     string power = \"battery\"\n\
                     string generation = \"solar\"\n\
                 }\n\
             )\n\
             {\n\
                 variantSet \"power\" = {\n\
                     \"battery\" {\n\
                         def Scope \"Bus\" {}\n\
                         def Scope \"Network\" (\n\
                             prepend apiSchemas = [\"CollectionAPI:components\"]\n\
                         )\n\
                         {\n\
                             uniform token collection:components:expansionRule = \"explicitOnly\"\n\
                             prepend rel collection:components:includes = [</Rover/Bus>]\n\
                         }\n\
                     }\n\
                 }\n\
                 variantSet \"generation\" = {\n\
                     \"solar\" {\n\
                         def Scope \"Panel\" {}\n\
                         over \"Network\"\n\
                         {\n\
                             prepend rel collection:components:includes = [</Rover/Panel>]\n\
                         }\n\
                     }\n\
                 }\n\
             }\n",
        )
        .expect("write rover");

        let scene = dir.path().join("variant_scene.usda");
        std::fs::write(
            &scene,
            "#usda 1.0\n\
             (\n\
                 defaultPrim = \"World\"\n\
             )\n\
             def Xform \"World\"\n\
             {\n\
                 def Xform \"Rig\" (\n\
                     prepend references = @./rover.usda@</Rover>\n\
                 )\n\
                 {\n\
                 }\n\
             }\n",
        )
        .expect("write variant scene");

        let members = members_of(&scene, "/World/Rig/Network");
        assert!(
            members.contains(&"/World/Rig/Panel".to_string()),
            "the `generation` variant's `prepend` did not reach the collection the \
             `power` variant defined — this is `generation = \"solar\"` on every \
             rocker-bogie rover; got {members:?}"
        );
        assert!(
            members.contains(&"/World/Rig/Bus".to_string()),
            "the `generation` variant's `prepend` REPLACED the `power` variant's \
             members instead of merging; got {members:?}"
        );
    }

    /// A later `over` must not erase the `def` it shares a layer and a path with.
    ///
    /// THE ACTUAL SOLAR BUG, minimised. Not list ops, not reference depth — the
    /// three tests above prove both of those compose correctly. Every broken
    /// solar asset writes the panel TWICE in one layer:
    ///
    ///     def Xform "SolarPanel" (
    ///         prepend references = @…/solar_panel.usda@</SolarPanel>
    ///     ) { … }
    ///     …
    ///     over "SolarPanel" {
    ///         custom token connectors:p.connect = </…/Battery.connectors:p>
    ///     }
    ///
    /// Two prim specs, one path, one layer. MEASURED on the shipped
    /// `scenes/luncosim/solar_rover_demo.usda`: the composed panel has NO children,
    /// NO `LunCoProgramAPI`, NO `info:sourceAsset`, and exactly one attribute —
    /// `connectors:p`, the one the `over` authored. The `over` won and the `def`,
    /// with its reference to the whole component, was dropped.
    ///
    /// The downstream symptom is a lie about a different thing entirely: the panel
    /// IS in the composed collection, but `read_network` skips members without
    /// `LunCoProgramAPI`, so `project_domain_islands` reports it as
    /// `outside collection` and rejects every solar rover in the repo.
    ///
    /// The sibling `over` is what a nearby prim is legitimately edited with, so
    /// this shape reads as normal. It is only wrong when it names a prim the SAME
    /// layer already `def`s — then the two opinions must merge, and the reference
    /// must survive.
    ///
    /// IGNORED, and the assets were fixed instead. This is a defect in the
    /// composition engine (the `openusd` fork), which this repo cannot fix from
    /// here; every shipped asset has been rewritten to one spec per prim, and
    /// `shipped_solar_panel_composes_its_component_reference` is the green gate
    /// that keeps them that way. Run it with `--ignored` when the fork is
    /// touched: the day it passes, this shape stops being a trap.
    #[test]
    #[ignore = "openusd fork: a later same-path `over` erases the `def`'s reference; assets are authored around it"]
    fn sibling_over_does_not_erase_a_def_in_the_same_layer() {
        let dir = tempfile::tempdir().expect("scratch dir");
        std::fs::write(
            dir.path().join("part.usda"),
            "#usda 1.0\n\
             (\n\
                 defaultPrim = \"Part\"\n\
             )\n\
             def Xform \"Part\" (\n\
                 prepend apiSchemas = [\"CollectionAPI:components\"]\n\
             )\n\
             {\n\
                 float inputs:rating = 42.0\n\
                 def Scope \"Guts\" {}\n\
             }\n",
        )
        .expect("write part");

        let scene = dir.path().join("assembly.usda");
        std::fs::write(
            &scene,
            "#usda 1.0\n\
             (\n\
                 defaultPrim = \"Assembly\"\n\
             )\n\
             def Xform \"Assembly\"\n\
             {\n\
                 def Xform \"Mounted\" (\n\
                     prepend references = @./part.usda@</Part>\n\
                 )\n\
                 {\n\
                 }\n\
                 over \"Mounted\"\n\
                 {\n\
                     custom token connectors:p\n\
                 }\n\
             }\n",
        )
        .expect("write assembly");

        let stage = compose_file_to_stage(&scene).expect("compose assembly");
        let view = StageView::new(&stage);
        let mounted = SdfPath::new("/Assembly/Mounted").unwrap();

        assert_eq!(
            view.value::<f32>(&mounted, "inputs:rating"),
            Some(42.0),
            "the sibling `over` erased the `def`'s reference — the referenced part's \
             own attribute is gone. Composed attrs: {:?}, children: {:?}",
            view.attr_names(&mounted),
            view.children(&mounted)
        );
        assert!(
            view.children(&mounted)
                .iter()
                .any(|c| c.as_str().ends_with("/Guts")),
            "the referenced part's child prim is missing; got {:?}",
            view.children(&mounted)
        );
        assert!(
            view.attr_names(&mounted)
                .iter()
                .any(|a| a == "connectors:p"),
            "the `over`'s own opinion is missing — the two specs must MERGE, not \
             pick a winner; got {:?}",
            view.attr_names(&mounted)
        );
    }

    /// The shipped solar rover's panel is a complete component, not a stub.
    ///
    /// The regression that motivated all of the above, asserted on the real
    /// asset rather than a fixture: `read_network` admits a collection member
    /// only if it carries `LunCoProgramAPI` and a `.mo` `info:sourceAsset`, and
    /// both of those arrive through the panel's reference. When the reference was
    /// being erased, the composed panel had ONE attribute (`connectors:p`), no
    /// children and no schemas — and the only symptom anywhere was a projection
    /// error naming the collection, which was never the thing that was wrong.
    #[test]
    fn shipped_solar_panel_composes_its_component_reference() {
        let stage = compose_file_to_stage(&asset("scenes/luncosim/solar_rover_demo.usda"))
            .expect("compose solar rover demo");
        let view = StageView::new(&stage);
        let panel = SdfPath::new("/SolarRoverTest/SolarRover/YawHead/SolarPanel").unwrap();

        assert!(
            view.has_api_schema(&panel, "LunCoProgramAPI"),
            "the panel lost `LunCoProgramAPI`, so `read_network` will skip it and the \
             electrical island will be rejected as `outside collection`. Composed \
             attrs: {:?}",
            view.attr_names(&panel)
        );
        assert!(
            view.asset(&panel, "info:sourceAsset").is_some(),
            "the panel lost its Modelica `info:sourceAsset`"
        );
        assert!(
            view.attr_names(&panel).iter().any(|a| a == "connectors:p"),
            "the panel lost its bus connection — folding the `over` into the `def` \
             must keep BOTH sets of opinions; got {:?}",
            view.attr_names(&panel)
        );
    }

    /// Composed `explicitOnly` members of `prim`, as plain strings.
    fn members_of(scene: &std::path::Path, prim: &str) -> Vec<String> {
        let stage = compose_file_to_stage(scene).expect("compose scene");
        let view = StageView::new(&stage);
        view.collection_members(&SdfPath::new(prim).unwrap(), "components")
            .expect("compute collection members")
            .into_iter()
            .map(|path| path.to_string())
            .collect()
    }
}
