extends Node

## Example script demonstrating rover joint control modes
## Attach this to a rover to see automated demonstrations of each mode

@export var controller_path: NodePath
@export var demo_duration: float = 5.0  # Seconds per demo
@export var auto_cycle_modes: bool = true

var controller: LCRoverJointController
var demo_timer: float = 0.0
var current_demo: int = 0

enum DemoMode {
	ACKERMANN_FORWARD,
	ACKERMANN_TURN,
	DIFFERENTIAL_FORWARD,
	DIFFERENTIAL_ROTATE,
	INDEPENDENT_CRAB,
	INDEPENDENT_DIAGONAL,
	IDLE
}

var demo_names = {
	DemoMode.ACKERMANN_FORWARD: "Ackermann: Forward Drive",
	DemoMode.ACKERMANN_TURN: "Ackermann: Turning",
	DemoMode.DIFFERENTIAL_FORWARD: "Differential: Forward Drive",
	DemoMode.DIFFERENTIAL_ROTATE: "Differential: Rotate in Place",
	DemoMode.INDEPENDENT_CRAB: "Independent: Crab Walk",
	DemoMode.INDEPENDENT_DIAGONAL: "Independent: Diagonal Movement",
	DemoMode.IDLE: "Idle"
}

func _ready():
	if controller_path.is_empty():
		controller = get_parent().get_node_or_null("RoverJointController")
	else:
		controller = get_node_or_null(controller_path)
	
	if not controller:
		push_error("RoverJointControlDemo: Could not find controller!")
		return
	
	print("=== Rover Joint Control Demo Started ===")
	print("Press number keys to activate demos:")
	print("  1 - Ackermann Forward")
	print("  2 - Ackermann Turn")
	print("  3 - Differential Forward")
	print("  4 - Differential Rotate")
	print("  5 - Independent Crab Walk")
	print("  6 - Independent Diagonal")
	print("  0 - Stop/Idle")
	print("  Space - Toggle Auto-Cycle")

func _process(delta):
	if not controller:
		return
	
	# Handle manual demo selection
	if Input.is_action_just_pressed("ui_text_completion_accept"):
		auto_cycle_modes = !auto_cycle_modes
		print("Auto-cycle: ", auto_cycle_modes)
	
	if Input.is_action_just_pressed("ui_text_completion_replace"):
		_start_demo(DemoMode.IDLE)
	
	# Number keys for manual demo selection
	for i in range(1, 7):
		if Input.is_key_label_pressed(KEY_0 + i):
			_start_demo(i - 1)
			auto_cycle_modes = false
	
	# Auto-cycle through demos
	if auto_cycle_modes:
		demo_timer += delta
		if demo_timer >= demo_duration:
			demo_timer = 0.0
			current_demo = (current_demo + 1) % 6
			_start_demo(current_demo)

func _start_demo(demo: int):
	current_demo = demo
	demo_timer = 0.0
	
	print("\n>>> Starting Demo: ", demo_names[demo], " <<<")
	
	# Reset all controls
	_reset_controls()
	
	# Execute demo
	match demo:
		DemoMode.ACKERMANN_FORWARD:
			_demo_ackermann_forward()
		DemoMode.ACKERMANN_TURN:
			_demo_ackermann_turn()
		DemoMode.DIFFERENTIAL_FORWARD:
			_demo_differential_forward()
		DemoMode.DIFFERENTIAL_ROTATE:
			_demo_differential_rotate()
		DemoMode.INDEPENDENT_CRAB:
			_demo_independent_crab()
		DemoMode.INDEPENDENT_DIAGONAL:
			_demo_independent_diagonal()
		DemoMode.IDLE:
			_demo_idle()

func _reset_controls():
	"""Reset all controls to neutral"""
	controller.set_motor(0.0)
	controller.set_steering(0.0)
	controller.set_brake(0.0)
	
	# Reset individual wheel controls
	for wheel in ["front_left", "front_right", "back_left", "back_right"]:
		controller.set_wheel_motor(wheel, 0.0)
		controller.set_wheel_brake(wheel, 0.0)
		controller.set_wheel_steering(wheel, 0.0)

# ============================================================================
# Demo Implementations
# ============================================================================

func _demo_ackermann_forward():
	"""Ackermann mode: Simple forward driving"""
	controller.drive_mode = 0  # Ackermann
	controller.enable_individual_control = false
	
	controller.set_motor(0.7)
	controller.set_steering(0.0)
	
	print("  Mode: Ackermann")
	print("  Action: Driving forward at 70% power")
	print("  All wheels receive same motor force")

func _demo_ackermann_turn():
	"""Ackermann mode: Forward with turning"""
	controller.drive_mode = 0  # Ackermann
	controller.enable_individual_control = false
	
	controller.set_motor(0.6)
	controller.set_steering(0.5)
	
	print("  Mode: Ackermann")
	print("  Action: Driving forward while turning right")
	print("  Front wheels steer, all wheels drive")

func _demo_differential_forward():
	"""Differential mode: Forward driving"""
	controller.drive_mode = 1  # Differential
	controller.enable_individual_control = false
	
	controller.set_motor(0.7)
	controller.set_steering(0.0)
	
	print("  Mode: Differential")
	print("  Action: Driving forward at 70% power")
	print("  Left and right wheels synchronized")

func _demo_differential_rotate():
	"""Differential mode: Rotate in place"""
	controller.drive_mode = 1  # Differential
	controller.enable_individual_control = false
	
	controller.set_motor(0.0)
	controller.set_steering(0.8)
	
	print("  Mode: Differential")
	print("  Action: Rotating in place (right)")
	print("  Left wheels forward, right wheels backward")
	print("  This is IMPOSSIBLE with Ackermann mode!")

func _demo_independent_crab():
	"""Independent mode: Crab walk (sideways movement)"""
	controller.drive_mode = 2  # Independent
	controller.enable_individual_control = true
	
	# All wheels same direction but with steering
	controller.set_wheel_motor("front_left", 0.5)
	controller.set_wheel_motor("front_right", 0.5)
	controller.set_wheel_motor("back_left", 0.5)
	controller.set_wheel_motor("back_right", 0.5)
	
	# Steer all wheels to the right
	controller.set_wheel_steering("front_left", 0.5)
	controller.set_wheel_steering("front_right", 0.5)
	
	print("  Mode: Independent")
	print("  Action: Crab walk (sideways movement)")
	print("  All wheels drive forward but steered right")
	print("  Creates diagonal/sideways motion")

func _demo_independent_diagonal():
	"""Independent mode: Diagonal movement"""
	controller.drive_mode = 2  # Independent
	controller.enable_individual_control = true
	
	# Diagonal movement pattern
	controller.set_wheel_motor("front_left", 0.8)
	controller.set_wheel_motor("front_right", 0.4)
	controller.set_wheel_motor("back_left", 0.4)
	controller.set_wheel_motor("back_right", 0.8)
	
	print("  Mode: Independent")
	print("  Action: Diagonal movement")
	print("  FL & BR: 80%, FR & BL: 40%")
	print("  Creates smooth diagonal trajectory")

func _demo_idle():
	"""Stop all movement"""
	_reset_controls()
	print("  Mode: Idle")
	print("  Action: All controls at neutral")
