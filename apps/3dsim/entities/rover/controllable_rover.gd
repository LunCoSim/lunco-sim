class_name LCControllableRover
extends RigidBody3D

signal control_granted
signal control_released

var _owner_id: int = 0

func _ready():
    # Set up networking and control
    set_multiplayer_authority(1)

func take_control(id: int) -> bool:
    if _owner_id != 0:
        return false
    
    _owner_id = id
    set_multiplayer_authority(id)
    control_granted.emit()
    return true

func release_control(id: int) -> bool:
    if _owner_id != id:
        return false
    
    _owner_id = 0
    set_multiplayer_authority(1)
    control_released.emit()
    return true

func get_owner_id() -> int:
    return _owner_id

# Ensure we have the required signals for the control system
func _get_configuration_warnings() -> PackedStringArray:
    var warnings = PackedStringArray()
    if not has_signal("control_granted") or not has_signal("control_released"):
        warnings.append("Missing required control signals")
    return warnings 