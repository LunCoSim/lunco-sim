class_name LCRoverInputAdapter
extends Node

@export var target: Node

# Input sensitivity and deadzone settings
@export var MOTOR_SENSITIVITY := 1.0
@export var STEERING_SENSITIVITY := 1.0
@export var INPUT_DEADZONE := 0.1

func _ready():
	# Try to find our controller if target is not set
	if not target:
		print("RoverInputAdapter: Target not set, looking for parent controller")
		# Try to find in parent first
		var parent = get_parent()
		if parent and parent.get_node_or_null("LCRoverController") != null:
			target = parent.get_node("LCRoverController")
			print("RoverInputAdapter: Found LCRoverController in parent: ", target)
		elif parent and parent.get_node_or_null("RoverController") != null:
			target = parent.get_node("RoverController")
			print("RoverInputAdapter: Found RoverController in parent: ", target)
		else:
			# Try to find any controller in the scene
			var potentialControllers = get_tree().get_nodes_in_group("RoverControllers")
			if potentialControllers.size() > 0:
				target = potentialControllers[0]
				print("RoverInputAdapter: Found controller in group: ", target)
				
			# Try to find the avatar to make it the target instead
			var avatar = get_node_or_null("/root/Avatar")
			if avatar:
				print("RoverInputAdapter: Found Avatar node, setting as target")
				target = avatar
	else:
		print("RoverInputAdapter: Target was already set to: ", target)
	
	# Create a timer to periodically check and report the state
	var timer = Timer.new()
	timer.wait_time = 2.0
	timer.one_shot = false
	timer.autostart = true
	timer.connect("timeout", Callable(self, "_on_timer_timeout"))
	add_child(timer)
	
	print("RoverInputAdapter: Ready, target is ", target)

func _on_timer_timeout():
	var _target = target
	
	# Report status for debugging
	if not target:
		print("RoverInputAdapter: No target set!")
		return
		
	if target is LCAvatar:
		_target = target.target
		print("RoverInputAdapter: Avatar's target is: ", _target)
	
	if _target is LCRoverController:
		print("RoverInputAdapter: Target is RoverController, is_controlled=", _target.is_controlled)
	else:
		# Try to look up the full avatar path
		var avatar_path = "???"
		if target:
			avatar_path = target.get_path()
		print("RoverInputAdapter: Target is not a RoverController: ", _target, " (avatar path: ", avatar_path, ")")

func _input(_event):
	# Only process inputs if we have a target
	if not target:
		return
		
	var _target = target
	
	# If target is an Avatar, get its current target
	if target is LCAvatar:
		_target = target.target
		
	# Check if we have a compatible rover controller
	var is_compatible_controller = _target is LCRoverController
	
	# Only process input if the target is a rover controller
	if is_compatible_controller:
		# Process movement input
		var motor_input = Input.get_action_strength("move_forward") - Input.get_action_strength("move_backward")
		var steering_input = Input.get_action_strength("move_right") - Input.get_action_strength("move_left")
		var brake_input = Input.get_action_strength("brake")
		
		# Process gamepad input if available
		var gamepad_movement = Input.get_vector("gamepad_left", "gamepad_right", "gamepad_forward", "gamepad_backward")
		if gamepad_movement.length() > INPUT_DEADZONE:
			motor_input = gamepad_movement.y if abs(gamepad_movement.y) > abs(motor_input) else motor_input
			steering_input = gamepad_movement.x if abs(gamepad_movement.x) > abs(steering_input) else steering_input
		
		# Apply inputs to controller
		if _target.has_method("set_motor"):
			_target.set_motor(motor_input * MOTOR_SENSITIVITY)
		
		if _target.has_method("set_steering"):
			_target.set_steering(steering_input * STEERING_SENSITIVITY)
		
		if _target.has_method("set_brake"):
			_target.set_brake(brake_input)
		
		# Print debug info - only for key presses to reduce spam
		if _event is InputEventKey and _event.pressed and not _event.echo:
			print("RoverInputAdapter: Input sent to ", _target.name) 
