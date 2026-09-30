fn scene_composition_admission(facts) {
    if facts.scene_path.index_of("reject.usda") >= 0
        || facts.scene_path.index_of("partial.usda") >= 0 {
        return #{ action: "reject_scene" };
    }
    #{ action: "allow_partial" }
}
