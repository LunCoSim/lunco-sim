"""Typed document-operation fixtures for scoped pause and prescribed receivers."""
from test_simulation_core_fifth_pass import actuation_ops, attribute, operation, raycast_ops


def pause_ops() -> tuple[list[dict], dict]:
    ops, params = raycast_ops("stationary")
    rover = params["rover_path"]
    physical = "/World/SixthPhysical"
    ops += [operation("SetTranslate", path=rover, value=[0.0, 30.0, 0.0]),
            attribute("/World/Physics", "physics:gravityMagnitude", "float", "1.62"),
            operation("SetActive", path="/World/ForceBody", active=True),
            operation("SetTranslate", path="/World/ForceBody", value=[30.0, 30.0, 0.0]),
            attribute("/World/ForceBody", "physxRigidBody:linearDamping", "float", "0"),
            operation("AddPrim", parent_path="/World", name="SixthPhysical", type_name="Xform",
                      reference="lunco://vessels/rovers/skid_rover.usda", reference_prim_path="/SkidRover"),
            operation("SetVariantSelection", path=physical, variant_set="drivetrain", variant="physical"),
            operation("SetTranslate", path=physical, value=[20.0, 30.0, 0.0])]
    return ops, {"rover_path": rover, "physical_path": physical,
                 "independent_path": "/World/ForceBody", "model_path": rover}


def prescribed_ops() -> tuple[list[dict], dict]:
    ops, params = actuation_ops("dual")
    params = {"body_path": params["body_path"], "actuator_path": params["mount_path"]}
    return ops, params
