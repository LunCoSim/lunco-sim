// Unarmed scene clicks share one semantic owner. This policy routes each click
// to an authored tool action or the generic selection command; engine
// adapters provide hit facts and validate/apply the resulting typed command.

fn on_pointer(context) {
    if context == () { return #{ ok: true, handled: false }; }

    let route = waypoint_editor::route_pointer_action(context);
    if route != () && route.handled == true { return route; }

    if context.button != "primary" { return #{ ok: true, handled: false }; }
    let replace = pointer_intent(context, "selection.replace");
    let extend = pointer_intent(context, "selection.extend");
    let remove = pointer_intent(context, "selection.remove");
    let drill = pointer_intent(context, "selection.drill");
    if !replace && !extend && !remove && !drill {
        return #{ ok: true, handled: false };
    }

    // In the simulation perspective, a plain click belongs to the avatar's
    // possessable-target resolver. Modified gestures remain explicit selection
    // intents. Editor perspective selection is entirely owned here.
    if context.scene_interaction_mode == "simulation" && replace {
        // Possession is a separate generic hit resolver. It may run only when
        // this is the sole semantic intent; an overlapping binding must have
        // one authored-policy owner instead of selecting and possessing from
        // the same physical gesture.
        if context.pointer_intents.len() != 1 {
            return #{ ok: true, handled: true, action: "intent_conflict" };
        }
        return #{ ok: true, handled: false, owner: "possession" };
    }

    let target = if context.target_entity_id != () {
        context.target_entity_id
    } else {
        context.hit_entity_id
    };
    if target == () { return #{ ok: true, handled: false }; }

    if drill && context.selected_entity_id == target
        && context.inspector_part_entity_id != () {
        cmd("SelectEntity", #{
            entity_id: target,
            inspector_part_entity_id: context.inspector_part_entity_id,
            extend: false,
            toggle: false,
            remove_only: false,
        });
        return #{ ok: true, handled: true, action: "inspect_part" };
    }

    cmd("SelectEntity", #{
        entity_id: target,
        inspector_part_entity_id: 0,
        extend: extend || drill,
        toggle: false,
        remove_only: remove,
    });
    #{ ok: true, handled: true, action: "select" }
}
