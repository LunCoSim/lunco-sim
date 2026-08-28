// Script-first assembly recipe. This is an authoring library: call
// `build_robot()` from an explicit RunRhai/RunScenario action after opening a
// USD stage. All writes go through the journaled command surface.
//
// Replace the asset paths with entries in your Twin. USD owns the assembly and
// joints; the Modelica batch below owns the controller graph.

const ROBOT = "/World/ScriptRover";
const MODEL  = "RoverControl";

fn build_robot(usd_doc, modelica_doc) {
    usd_add_prim(usd_doc, "@root@", "/World", "ScriptRover", "Xform", ());
    usd_set_translate(usd_doc, "@root@", ROBOT, [0.0, 0.0, 0.0]);

    // A fixed sensor mount and a revolute wheel are both authored through the
    // same AttachComponent contract used by the editor and HTTP API.
    attach_fixed(
        usd_doc,
        "@root@",
        ROBOT,
        "NavSensor",
        "constraint_sensor",
        "components/sensors/nav_sensor.usda",
        [0.0, 1.0, 0.0],
    );
    attach_revolute(
        usd_doc,
        "@root@",
        ROBOT,
        "Wheel_FL",
        "constraint_wheel_fl",
        "components/mobility/wheel.usda",
        [0.8, -0.35, 0.9],
        [0.0, 0.0, 0.0],
        "X",
    );

    // One batch is one Modelica command and each ApiOp is the canonical
    // structural edit; no raw .mo text parser is embedded in this script.
    modelica_apply(modelica_doc, [
        modelica_add_component(
            MODEL,
            "Modelica.Blocks.Sources.Constant",
            "throttleBias",
            modelica_placement(-260.0, 40.0, 80.0, 40.0),
            [modelica_modification("k", "0.0")],
        ),
        modelica_add_component(
            MODEL,
            "Modelica.Blocks.Math.Gain",
            "driveGain",
            modelica_placement(-80.0, 40.0, 80.0, 40.0),
            [modelica_modification("k", "1.0")],
        ),
        modelica_connect(MODEL, "throttleBias", "y", "driveGain", "u"),
    ]);
}

// Call build_robot with the USD and Modelica document ids returned by the
// workspace API. Explicit ids keep the scene and equation graph authoritative
// when more than one document is open.
