//! Lesson 5 — Rhai Editor & REPL. Learn how to write scripts in the editor and use the REPL.

fn steps() {
    [
        #{ anchor: "",             focus: (),              title: "Rhai Policy",         body: "Welcome to Rhai policy in the Sandbox. Use it for event-driven sequencing, mission objectives, and scenario glue; continuous controller equations belong in Modelica or Rust. Let's look at the tools. Next / Back below." },
        #{ anchor: "panel.center", focus: "rhai_editor",   title: "The Rhai Editor",     body: "We opened the Behaviour editor. Here you can edit `.rhai` script files directly inside LunCoSim with full syntax highlighting. These scripts are attached to USD entities to define their dynamic behaviour." },
        #{ anchor: "panel.bottom", focus: "rhai_repl",     title: "The Live REPL",       body: "At the bottom dock is the Rhai REPL (Read-Eval-Print Loop) console. Type commands here like `print(3 + 4)` or dispatch commands to control rovers instantly in the running simulation." },
        #{ anchor: "panel.bottom", focus: "rhai_repl",     title: "Parameter Hints",     body: "The REPL features auto-completion and USD parameter hints to make typing commands and query names quick. This is the same scripting engine that runs behind our AI agents and MCP tools." },
        #{ anchor: "",             focus: (),              title: "Go deeper",           body: "Now you know where the script editor and REPL live. Open them up and try typing a command in the console to automate your next mission!" },
    ]
}

fn show(s, i) {
    coach_step(s, i);
}

fn on_start(me) {
    // One source: the lesson sets up its own environment. Without this the
    // coach marks landed on top of whatever scene happened to be open, so
    // "open the tutorial" changed nothing but the overlay. Same rover scene as
    // the neighbouring script/inspect lessons, so the REPL examples below have
    // a live entity to talk to.
    // The world this lesson teaches in is DECLARED in the curriculum layer
    // (a `payload` arc) and mounted by the launcher before this runs.
    this.i = 0;
    show(steps(), 0);
    notify_kind("Tutorial: Rhai Editor & REPL", "info");
}

fn on_event(me, evt) {
    let s = steps();
    if evt.name == "cmd:TutorialNext"      { this.i += 1; }
    else if evt.name == "cmd:TutorialBack" { if this.i > 0 { this.i -= 1; } }
    else if evt.name == "cmd:TutorialSkip" { this.i = s.len(); }
    else if evt.name == "cmd:TutorialGoto" { this.i = evt.value; }
    else { return; }

    if this.i >= s.len() {
        end_tour();
        notify_kind("Rhai Editor & REPL tutorial complete! 🎉", "success");
        emit("MISSION_COMPLETE", 0);
    } else {
        show(s, this.i);
    }
}
