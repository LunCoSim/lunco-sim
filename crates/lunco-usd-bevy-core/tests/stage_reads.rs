#![cfg(not(target_arch = "wasm32"))]

//! Integration coverage for the public composed-stage read boundary. These
//! tests stay outside the production library target so changes to fixtures and
//! read assertions do not rebuild the core library.

use anyhow::Result;
use lunco_assets_core::asset_path::{canonicalize, canonicalize_root};
use lunco_usd_bevy_core::compose::build_stage_with_resolver;
use lunco_usd_bevy_core::read::runtime_port_provider;
use lunco_usd_bevy_core::{StageView, UsdRead};
use lunco_usd_compose::recipe::StageRecipe;
use openusd::usd::Stage;
use std::collections::HashMap;

fn build_stage_from_closure(recipe: &StageRecipe) -> Result<Stage> {
    build_stage_with_resolver(recipe).map(|(stage, _)| stage)
}

fn recipe_with_layers(root_id: &str, root_source: &str, layers: &[(&str, &str)]) -> StageRecipe {
    let root_id = canonicalize_root(root_id);
    let mut bytes = HashMap::from([(root_id.clone(), root_source.as_bytes().to_vec())]);
    for (layer_id, source) in layers {
        bytes.insert(canonicalize(layer_id, &root_id), source.as_bytes().to_vec());
    }
    StageRecipe::new(root_id, bytes)
}

mod inherits_compose_tests {
    use super::*;
    use openusd::sdf::Path as SdfPath;

    /// De-risk the control-profile design: a `class` carrying a `Controls` child
    /// scope, `inherits`-ed by a vessel prim, must land those child prims (with
    /// their attrs) under the vessel after full PCP flatten — so the entity
    /// translator can walk `<Vessel>/Controls/<intent>` to build a `ControlBinding`.
    #[test]
    fn inherits_from_class_brings_child_prims_into_flattened_data() {
        let usda = "#usda 1.0\n\
class \"_RoverControl\"\n{\n    def \"Controls\"\n    {\n        def \"forward\"\n        {\n            uniform string lunco:port = \"throttle\"\n            uniform double lunco:factor = 1\n        }\n    }\n}\n\
def Xform \"Rover\" (\n    inherits = </_RoverControl>\n)\n{\n}\n";
        let stage = build_stage_from_closure(&lunco_usd_compose::recipe::StageRecipe::from_source(
            "inherits.usda",
            usda,
        ))
        .expect("compose");
        let view = StageView::new(&stage);
        let fwd = SdfPath::new("/Rover/Controls/forward").unwrap();
        assert_eq!(
            view.value::<String>(&fwd, "lunco:port").as_deref(),
            Some("throttle"),
            "inherited Controls child must appear under /Rover with its attrs"
        );
        assert_eq!(view.value::<f64>(&fwd, "lunco:factor"), Some(1.0));
    }

    /// The real delivery mechanism: a vessel in one file pulls a control-profile
    /// `class` from ANOTHER file via `subLayers`, then `inherits` it — the
    /// `Controls` child scope must compose onto the vessel. Proves rovers/landers
    /// can share one profile file (DRY) without repeating bindings per asset.
    #[test]
    fn cross_file_sublayer_inherits_composes() {
        let profile = "#usda 1.0\nclass \"_RoverControl\"\n{\n    def \"Controls\"\n    {\n        def \"forward\"\n        {\n            uniform string lunco:port = \"throttle\"\n            uniform double lunco:factor = 1\n        }\n    }\n}\n";
        let rover = "#usda 1.0\n(\n    subLayers = [@./control_profiles.usda@]\n)\ndef Xform \"SkidRover\" (\n    inherits = </_RoverControl>\n)\n{\n}\n";
        let recipe = recipe_with_layers("rover.usda", rover, &[("control_profiles.usda", profile)]);
        let stage = build_stage_from_closure(&recipe).expect("compose stage");
        let view = StageView::new(&stage);
        let fwd = SdfPath::new("/SkidRover/Controls/forward").unwrap();
        assert_eq!(
            view.value::<String>(&fwd, "lunco:port").as_deref(),
            Some("throttle"),
            "cross-file subLayers+inherits must land the Controls scope on the vessel"
        );
    }

    /// A binary `payload` authored inside a REFERENCED `.usda` wrapper must be
    /// found on the COMPOSED prim (`/Scene/Bldg/Visual`), with its URI anchored
    /// at the wrapper layer. This keeps USD as the source of truth —
    /// `scene → .usda → .glb` — while the render projection reads the live arc.
    #[test]
    fn glb_payload_in_referenced_wrapper_anchors_on_composed_prim() {
        // Wrapper: a `Structure` defaultPrim whose `Visual` child carries the glb
        // payload — the Perseverance "usda → glb" shape.
        let wrapper = "#usda 1.0\n(\n    defaultPrim = \"Structure\"\n)\ndef Xform \"Structure\"\n{\n    def Xform \"Visual\" (\n        prepend payload = @model.glb@\n    )\n    {\n        string lunco:assetMode = \"scene\"\n    }\n}\n";
        // Scene references the wrapper — no direct glb embedding in the scene.
        let scene = "#usda 1.0\ndef Xform \"Scene\"\n{\n    def Xform \"Bldg\" (\n        prepend references = @wrapper.usda@\n    )\n    {\n    }\n}\n";
        // Build the two-layer closure keyed exactly as the async loader's resolver
        // does (`canonicalize`), so the scene's `@wrapper.usda@` reference resolves
        // to the wrapper bytes and the `@model.glb@` payload is stubbed — the
        // storage-based compose path, not the removed native-fs path.
        let root_id = canonicalize_root("scene.usda");
        let wrapper_id = lunco_assets_core::asset_path::canonicalize("wrapper.usda", &root_id);
        let bytes = HashMap::from([
            (root_id.clone(), scene.as_bytes().to_vec()),
            (wrapper_id, wrapper.as_bytes().to_vec()),
        ]);
        let stage =
            build_stage_from_closure(&lunco_usd_compose::recipe::StageRecipe::new(root_id, bytes))
                .expect("compose scene→wrapper→glb");
        let view = StageView::new(&stage);

        let visual = SdfPath::new("/Scene/Bldg/Visual").unwrap();
        let resolved = view
            .binary_asset_uri(&visual)
            .expect("binary payload must be read from the composed Visual prim");
        assert!(
            resolved.ends_with("model.glb"),
            "binary asset URI should point at the wrapper-co-located glb, got {resolved}"
        );
    }

    #[test]
    fn binary_asset_uri_rejects_multiple_authored_binary_arcs() {
        let source = "#usda 1.0\n\
def Xform \"Visual\" (\n\
    prepend payload = @model.glb@\n\
    prepend references = @alternate.glb@\n\
)\n\
{\n\
}\n";
        let stage = build_stage_from_closure(&lunco_usd_compose::recipe::StageRecipe::from_source(
            "scene.usda",
            source,
        ))
        .expect("compose binary arcs");
        let view = StageView::new(&stage);
        let visual = SdfPath::new("/Visual").unwrap();

        assert!(
            view.binary_asset_uri(&visual).is_none(),
            "ambiguous binary arcs must not choose an arbitrary asset"
        );
    }
}
mod stage_view_tests {
    //! Live-stage composition reads through `StageView` (the openusd composed
    //! Stage): the cross-file inherit/subLayer opinion the entity translator
    //! consumes must land on the vessel after full PCP composition.

    use super::{build_stage_from_closure, recipe_with_layers, StageRecipe, StageView, UsdRead};
    use openusd::sdf::Path as SdfPath;
    #[test]
    fn collection_members_uses_standard_subtree_expansion() {
        let source = "#usda 1.0\n\
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
             }\n";
        let recipe = StageRecipe::from_source("collection.usda", source);
        let stage = build_stage_from_closure(&recipe).expect("compose collection scene");
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
        // The base: the shape of `assets/scenes/luncosim/lander_ops.usda`.
        let base = "#usda 1.0\n\
             def Xform \"Base\"\n\
             {\n\
                 def Cube \"Ground\"\n\
                 {\n\
                     float lunco:test:albedo = 0.35\n\
                 }\n\
             }\n";

        // The episode: reference the base, then override that ground's look.
        let episode = "#usda 1.0\n\
             def Xform \"Episode\" (\n\
                 prepend references = @./base.usda@</Base>\n\
             )\n\
             {\n\
                 over \"Ground\"\n\
                 {\n\
                     float lunco:test:albedo = 0.13\n\
                 }\n\
             }\n";

        let recipe = recipe_with_layers("episode.usda", episode, &[("base.usda", base)]);
        let stage = build_stage_from_closure(&recipe).expect("compose episode");
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
    //     output source component `…/SolarPanel` is outside the rover-root collection
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
        let source = "#usda 1.0\n\
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
             }\n";
        let recipe = StageRecipe::from_source("flat.usda", source);
        let members = members_of(&recipe, "/World/Network");
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
    ///     over "Rover" {
    ///         prepend rel collection:components:includes = [</…/SolarPanel>]
    ///     }
    ///
    /// A `prepend` that REPLACES instead of merging loses the six motors and the
    /// battery; a `prepend` that is DROPPED loses the panel — which is what the
    /// runtime error reports. Both are asserted, so the failure message says which
    /// happened rather than only that the set was wrong.
    #[test]
    fn stronger_layer_prepend_merges_into_referenced_collection() {
        let base = "#usda 1.0\n\
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
             }\n";

        let scene = "#usda 1.0\n\
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
             }\n";

        let recipe = recipe_with_layers("scene.usda", scene, &[("base.usda", base)]);
        let members = members_of(&recipe, "/World/Rig/Network");
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
    /// the rover root with the bus members, and `generation = "solar"` OVERS it to
    /// add the panel. Two sibling variant arcs on the same prim, both authoring
    /// the same relationship — and the selection is made a further arc away, in a
    /// profile asset or a scene, so this is the composition that has to hold for
    /// any purchasable rover configuration to work.
    #[test]
    fn sibling_variant_prepends_merge_into_one_collection() {
        let rover = "#usda 1.0\n\
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
             }\n";

        let scene = "#usda 1.0\n\
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
             }\n";

        let recipe = recipe_with_layers("variant_scene.usda", scene, &[("rover.usda", rover)]);
        let members = members_of(&recipe, "/World/Rig/Network");
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
    /// The production scene/Rhai gates cover the shipped asset wiring. Run this
    /// synthetic seam when the OpenUSD fork is touched: the day it passes, this
    /// shape stops being a trap.
    #[test]
    #[ignore = "openusd fork: a later same-path `over` erases the `def`'s reference; assets are authored around it"]
    fn sibling_over_does_not_erase_a_def_in_the_same_layer() {
        let part = "#usda 1.0\n\
             (\n\
                 defaultPrim = \"Part\"\n\
             )\n\
             def Xform \"Part\" (\n\
                 prepend apiSchemas = [\"CollectionAPI:components\"]\n\
             )\n\
             {\n\
                 float inputs:rating = 42.0\n\
                 def Scope \"Guts\" {}\n\
             }\n";

        let scene = "#usda 1.0\n\
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
             }\n";

        let recipe = recipe_with_layers("assembly.usda", scene, &[("part.usda", part)]);
        let stage = build_stage_from_closure(&recipe).expect("compose assembly");
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

    /// Composed `explicitOnly` members of `prim`, as plain strings.
    fn members_of(recipe: &StageRecipe, prim: &str) -> Vec<String> {
        let stage = build_stage_from_closure(recipe).expect("compose scene");
        let view = StageView::new(&stage);
        view.collection_members(&SdfPath::new(prim).unwrap(), "components")
            .expect("compute collection members")
            .into_iter()
            .map(|path| path.to_string())
            .collect()
    }
}
mod real_reader_tests {
    //! The precision-tolerant [`real`](UsdRead::real) family reads a numeric
    //! value regardless of whether it was authored `float`, `double`, `int`,
    //! or `int64`. This is the guard against the silent-fallback bug:
    //! strict scalar reads match only one USD type and silently drop the rest.

    use super::UsdRead;
    use lunco_usd_bevy_core::compose::build_stage_with_resolver;
    use lunco_usd_bevy_core::StageView;
    use lunco_usd_compose::recipe::StageRecipe;
    use openusd::sdf::{Path as SdfPath, Value};
    use openusd::usd::Stage;

    const SCENE: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

    /// Build a live stage carrying a `float`-authored and a `double`-authored
    /// attribute on `/World`.
    struct TestStage(Stage);

    impl TestStage {
        fn stage(&self) -> &Stage {
            &self.0
        }

        fn view(&self) -> StageView<'_> {
            StageView::new(&self.0)
        }
    }

    fn test_stage_from_recipe(recipe: &StageRecipe) -> TestStage {
        TestStage(build_stage_with_resolver(recipe).expect("stage builds").0)
    }

    fn stage_with_mixed_precision() -> TestStage {
        let cs = test_stage_from_recipe(&StageRecipe::from_source("scene.usda", SCENE));
        let stage = cs.stage();
        stage
            .create_attribute("/World.f_val", "float")
            .unwrap()
            .set(Value::Float(2.5))
            .unwrap();
        stage
            .create_attribute("/World.d_val", "double")
            .unwrap()
            .set(Value::Double(3.5))
            .unwrap();
        stage
            .create_attribute("/World.i_val", "int")
            .unwrap()
            .set(Value::Int(4))
            .unwrap();
        stage
            .create_attribute("/World.i64_val", "int64")
            .unwrap()
            .set(Value::Int64(5))
            .unwrap();
        stage
            .create_attribute("/World.bool_val", "bool")
            .unwrap()
            .set(Value::Bool(true))
            .unwrap();
        stage
            .create_attribute("/World.int_flag", "int64")
            .unwrap()
            .set(Value::Int64(1))
            .unwrap();
        cs
    }

    #[test]
    fn asset_reads_an_authored_asset_path_off_the_live_stage() {
        // The exact production case that was in doubt: an `asset`-typed attribute
        // (`lunco:layer:demSource`, a policy's `info:sourceAsset`) read off a live COMPOSED
        // stage. `scalar::<String>` / `text` must NOT read it (it's `Value::AssetPath`,
        // not String/Token); `asset` must return the authored `@…@` path.
        const S: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\
            def Xform \"World\"\n{\n    asset a_val = @terrain/connecting_ridge@\n}\n";
        let cs = test_stage_from_recipe(&StageRecipe::from_source("scene.usda", S));
        let view = cs.view();
        let world = SdfPath::new("/World").unwrap();
        assert_eq!(
            view.asset(&world, "a_val").as_deref(),
            Some("terrain/connecting_ridge"),
            "asset() reads the authored @…@ path off the composed stage"
        );
        // A strict typed `String` read misses it — the value is `Value::AssetPath`.
        assert_eq!(
            view.scalar::<String>(&world, "a_val"),
            None,
            "a String read misses an asset"
        );
        // `text` coerces via `as_str` (same as `upAxis`), so it ALSO yields the path —
        // which is why the pre-migration `string demSource` read worked; the asset
        // migration is about the type contract, not about making the read possible.
        assert_eq!(
            view.text(&world, "a_val").as_deref(),
            Some("terrain/connecting_ridge")
        );
    }

    #[test]
    fn runtime_provider_recognizes_standard_joint_prims() {
        const JOINTS: &str = r#"#usda 1.0
(
    defaultPrim = "World"
)
def Xform "World"
{
    def PhysicsRevoluteJoint "Hinge"
    {
    }
    def PhysicsPrismaticJoint "Slider"
    {
    }
    def Xform "Plain"
    {
    }
}
"#;
        let cs = test_stage_from_recipe(&StageRecipe::from_source("scene.usda", JOINTS));
        let view = cs.view();
        let hinge = SdfPath::new("/World/Hinge").unwrap();
        let slider = SdfPath::new("/World/Slider").unwrap();
        let plain = SdfPath::new("/World/Plain").unwrap();
        let missing = SdfPath::new("/World/Missing").unwrap();

        assert_eq!(
            super::runtime_port_provider(&view, &hinge),
            Some("PhysicsRevoluteJoint")
        );
        assert_eq!(
            super::runtime_port_provider(&view, &slider),
            Some("PhysicsPrismaticJoint")
        );
        assert_eq!(
            super::runtime_port_provider(&view, &plain),
            None,
            "an ordinary prim is not a runtime port provider"
        );
        assert_eq!(
            super::runtime_port_provider(&view, &missing),
            None,
            "a missing source prim never becomes a runtime provider"
        );
    }

    #[test]
    fn attr_type_name_preserves_usd_roles_and_array_shape() {
        let source = r#"#usda 1.0
(
    defaultPrim = "World"
)
def Xform "World"
{
    color3f scalar_color = (1, 0, 0)
    color3f[] array_color = [(0, 1, 0)]
    point3f point = (1, 2, 3)
    vector3f direction = (0, 0, 1)
}
"#;
        let cs = test_stage_from_recipe(&StageRecipe::from_source("scene.usda", source));
        let view = cs.view();
        let world = SdfPath::new("/World").unwrap();

        assert_eq!(
            view.attr_type_name(&world, "scalar_color").as_deref(),
            Some("color3f")
        );
        assert_eq!(
            view.attr_type_name(&world, "array_color").as_deref(),
            Some("color3f[]")
        );
        assert_eq!(
            view.attr_type_name(&world, "point").as_deref(),
            Some("point3f")
        );
        assert_eq!(
            view.attr_type_name(&world, "direction").as_deref(),
            Some("vector3f")
        );
    }

    #[test]
    fn points2_reads_either_authored_uv_precision() {
        // `primvars:st` is `texCoord2f[]` from Blender and `texCoord2d[]` from Maya /
        // Houdini. A strict `2f` read of the `2d` spelling reports "no UVs", and the
        // mesh builder answers that with a ZEROED UV set — the whole surface then
        // samples its texture at (0,0) and renders flat, which misreads as a material
        // bug rather than a type mismatch.
        const S: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\n\
            def Xform \"World\"\n{\n\
            \x20   texCoord2f[] st_f = [(0, 0), (1, 0), (1, 1)]\n\
            \x20   texCoord2d[] st_d = [(0, 0), (1, 0), (1, 1)]\n}\n";
        let cs = test_stage_from_recipe(&StageRecipe::from_source("scene.usda", S));
        let view = cs.view();
        let world = SdfPath::new("/World").unwrap();

        // The bug this exists to prevent: the strict read drops the double spelling.
        assert_eq!(
            view.scalar::<Vec<[f32; 2]>>(&world, "st_d"),
            None,
            "strict texCoord2f[] read drops a texCoord2d[] UV set"
        );

        let want = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        assert_eq!(
            view.points2(&world, "st_f"),
            want,
            "points2 reads float UVs"
        );
        assert_eq!(
            view.points2(&world, "st_d"),
            want,
            "points2 reads double UVs"
        );
        // Tolerance is not fabrication: an absent attribute is still empty.
        assert!(
            view.points2(&world, "missing").is_empty(),
            "absent attr stays empty"
        );
    }

    #[test]
    fn real_family_reads_either_authored_precision() {
        let cs = stage_with_mixed_precision();
        let view = cs.view();
        let world = SdfPath::new("/World").unwrap();

        // The bug this family exists to prevent: a strict typed read of the
        // *other* precision silently yields `None`.
        assert_eq!(
            view.scalar::<f64>(&world, "f_val"),
            None,
            "strict f64 read drops a float-authored value — the silent fallback bug"
        );
        assert_eq!(
            view.scalar::<f32>(&world, "d_val"),
            None,
            "strict f32 read drops a double-authored value"
        );

        // `real` (→ f64) reads BOTH a float- and a double-authored opinion.
        assert_eq!(view.real(&world, "f_val"), Some(2.5), "real reads float");
        assert_eq!(view.real(&world, "d_val"), Some(3.5), "real reads double");
        assert_eq!(view.real(&world, "i_val"), Some(4.0), "real reads int");
        assert_eq!(view.real(&world, "i64_val"), Some(5.0), "real reads int64");

        // `real_f32` (→ f32) likewise reads either precision.
        assert_eq!(
            view.real_f32(&world, "d_val"),
            Some(3.5),
            "real_f32 reads double"
        );
        assert_eq!(
            view.real_f32(&world, "f_val"),
            Some(2.5),
            "real_f32 reads float"
        );
        assert_eq!(view.real_f32(&world, "i_val"), Some(4.0));
        assert_eq!(view.real_f32(&world, "i64_val"), Some(5.0));
        assert_eq!(view.boolean(&world, "bool_val"), Some(true));
        assert_eq!(view.boolean(&world, "int_flag"), Some(true));
        assert_eq!(view.boolean(&world, "i_val"), Some(true));

        // The time-sampled variants fall back to the `default` opinion when a
        // channel has no `timeSamples`, and are precision-tolerant there too.
        assert_eq!(
            view.real_at(&world, "f_val", 0.0),
            Some(2.5),
            "real_at reads float default"
        );
        assert_eq!(
            view.real_f32_at(&world, "d_val", 0.0),
            Some(3.5),
            "real_f32_at reads double default"
        );
        assert_eq!(view.real_at(&world, "i_val", 0.0), Some(4.0));
        assert_eq!(view.real_f32_at(&world, "i64_val", 0.0), Some(5.0));

        // A genuinely absent attribute is still `None` (tolerance ≠ fabrication).
        assert_eq!(view.real(&world, "missing"), None, "absent attr stays None");
    }

    #[test]
    fn authored_attribute_presence_is_separate_from_composed_value() {
        let cs = test_stage_from_recipe(&StageRecipe::from_source("scene.usda", SCENE));
        cs.stage()
            .create_attribute("/World.authored", "float")
            .unwrap()
            .set(Value::Float(1.0))
            .unwrap();
        let view = cs.view();
        let world = SdfPath::new("/World").unwrap();
        assert!(view.has_authored_attribute(&world, "authored"));
        assert!(!view.has_authored_attribute(&world, "missing"));
    }

    #[test]
    fn authored_attribute_presence_includes_selected_variant_properties() {
        let source = r#"#usda 1.0
(
    defaultPrim = "World"
)
def Xform "World" (
    variants = { string site = "apollo" }
    prepend variantSets = "site"
)
{
    variantSet "site" = {
        "apollo" {
            double lunco:anchor:lat = 26.0371
            double lunco:anchor:lon = 3.6584
        }
    }
}
"#;
        let cs = test_stage_from_recipe(&StageRecipe::from_source("scene.usda", source));
        let view = cs.view();
        let world = SdfPath::new("/World").unwrap();

        assert_eq!(view.real(&world, "lunco:anchor:lat"), Some(26.0371));
        assert!(view.has_authored_attribute(&world, "lunco:anchor:lat"));
        assert!(view.has_authored_attribute(&world, "lunco:anchor:lon"));
    }

    #[test]
    fn asset_typed_attribute_reads_as_asset_and_not_as_string() {
        let source = "#usda 1.0\n\
            def Shader \"Shader\"\n{\n\
            uniform token info:implementationSource = \"sourceAsset\"\n\
            uniform asset info:wgsl:sourceAsset = @shaders/wheel.wgsl@\n}\n";
        let stage = test_stage_from_recipe(&StageRecipe::from_source("shader.usda", source));
        let view = stage.view();
        let shader = SdfPath::new("/Shader").unwrap();

        assert_eq!(
            UsdRead::asset(&view, &shader, "info:wgsl:sourceAsset").as_deref(),
            Some("shaders/wheel.wgsl"),
        );
        assert!(
            view.scalar::<String>(&shader, "info:wgsl:sourceAsset")
                .is_none(),
            "an asset must not be accepted as a String"
        );
        assert_eq!(
            UsdRead::text(&view, &shader, "info:implementationSource").as_deref(),
            Some("sourceAsset"),
        );
        assert!(
            view.scalar::<String>(&shader, "info:implementationSource")
                .is_none(),
            "a token must not be accepted as a String"
        );
    }

    #[test]
    fn resolve_stage_prim_path_uses_composed_default_prim_for_empty_mounts() {
        let source = "#usda 1.0\n(\n    defaultPrim = \"Apollo\"\n)\ndef Xform \"Apollo\"\n{\n}\n";
        let cs = test_stage_from_recipe(&StageRecipe::from_source("scene.usda", source));
        let view = cs.view();

        assert_eq!(
            lunco_usd_bevy_core::resolve_stage_prim_path(&view, ""),
            Some("/Apollo".into())
        );
        assert_eq!(
            lunco_usd_bevy_core::resolve_stage_prim_path(&view, "/Apollo/Embodiment"),
            Some("/Apollo/Embodiment".into())
        );
    }
}
