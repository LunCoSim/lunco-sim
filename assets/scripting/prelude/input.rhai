// ── Semantic pointer input ─────────────────────────────────────────────────

/// True when the generic pointer context carries the named configured intent.
/// Physical buttons and modifier keys are resolved by InputBindingsSettings;
/// authored tools consume this semantic surface instead of reimplementing a
/// keymap or hardcoding a gesture.
fn pointer_intent(context, name) {
    if context == () || type_of(name) != "string" || name == "" {
        return false;
    }
    let intents = context.pointer_intents;
    if intents == () { return false; }
    intents.contains(name)
}

// ── Native input automation ────────────────────────────────────────────────
// These wrappers enqueue typed Bevy window-input messages. They do not call a
// scene tool directly: picking, egui, focus, keymaps, selection, and authored
// tools receive the same event path as hardware input. Compose them in Rhai to
// express a gesture or workflow (for example: press Alt, click terrain,
// release Alt). The event maps use the reflected enum's externally tagged
// shape, so Rhai and API callers share exactly the same typed contract.
fn input_state_name(state) {
    if state == "pressed" { return "Pressed"; }
    if state == "released" { return "Released"; }
    state
}

fn input_button_name(button) {
    if button == "primary" || button == "left" { return "Primary"; }
    if button == "secondary" || button == "right" { return "Secondary"; }
    if button == "middle" { return "Middle"; }
    button
}

fn input_key(key, state) {
    cmd("InjectWindowInput", #{ event: #{
        Key: #{
            key: key,
            state: input_state_name(state),
            text: (),
            repeat: false
        }
    }})
}

fn input_key_press(key)   { input_key(key, "pressed") }
fn input_key_release(key) { input_key(key, "released") }

fn input_pointer_move(x, y) {
    cmd("InjectWindowInput", #{ event: #{
        PointerMove: #{ x: x, y: y }
    }})
}

fn input_pointer_button(button, state, x, y) {
    cmd("InjectWindowInput", #{ event: #{
        PointerButton: #{
            button: input_button_name(button),
            state: input_state_name(state),
            x: x,
            y: y
        }
    }})
}

fn input_pointer_press(button, x, y) {
    input_pointer_button(button, "pressed", x, y)
}

fn input_pointer_release(button, x, y) {
    input_pointer_button(button, "released", x, y)
}

fn input_click(button, x, y) {
    input_pointer_press(button, x, y);
    input_pointer_release(button, x, y)
}

fn input_scroll(x, y) {
    cmd("InjectWindowInput", #{ event: #{
        Scroll: #{ x: x, y: y }
    }})
}
