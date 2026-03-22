class_name WheelVisualController
extends Node3D

@onready var wheel_effector: LCWheelEffector = get_parent()
var _accumulated_angle: float = 0.0

func _physics_process(delta: float) -> void:
    if wheel_effector.is_in_contact():
        # Physics engine handles rotation when in contact
        # Reset our manual angle to track with physics state
        _accumulated_angle = rotation.x
        return
        
    # Manual rotation when airborne
    # Using cached RPM or calculating from torque request if RPM is zero/low
    var rpm = wheel_effector.get_wheel_rpm()
    
    # If stationary in air, simulate slight spin-down based on last RPM
    var angular_velocity = rpm * TAU / 60.0
    _accumulated_angle += angular_velocity * delta
    
    # Rotate this node (the mesh parent)
    rotation.x = _accumulated_angle
