class_name LCRoverInputAdapter
extends Node

@export var target: Node

# Input deadzone
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
	
	print("RoverInputAdapter: Ready, target is ", target)

func _input(_event):
	var _target = target
	# Handle avatar target case
	if target is LCAvatar:
		_target = target.target
	
	# Check if we have a compatible rover controller
	var is_rover_controller = _target is LCRoverController
	var is_3dsim_rover_controller = _target.get_script() and _target.get_script().get_path().find("rover_controller.gd") != -1
	
	# Debug key press
	if _event is InputEventKey and _event.pressed and not _event.echo:
		if is_rover_controller or is_3dsim_rover_controller:
			print("RoverInputAdapter: Processing key input for rover controller", _target)
		else:
			print("RoverInputAdapter: Target is not a rover controller: ", _target, " type: ", _target.get_class())
	
	# Process input for any rover controller
	if is_rover_controller or is_3dsim_rover_controller:
		# Handle forward/reverse movement
		var motor_input = Input.get_action_strength("move_forward") - Input.get_action_strength("move_backward")
		
		# Handle steering
		var steering_input = Input.get_action_strength("move_right") - Input.get_action_strength("move_left")
		
		# Handle braking
		var brake_input = Input.get_action_strength("brake")
		
		# Process gamepad input if available
		var gamepad_movement = Input.get_vector("gamepad_left", "gamepad_right", "gamepad_forward", "gamepad_backward")
		if gamepad_movement.length() > 0.1:
			motor_input = gamepad_movement.y
			steering_input = gamepad_movement.x
		
		# Apply the inputs to the controller (both controller types have the same methods)
		_target.set_motor(motor_input)
		_target.set_steering(steering_input)
		_target.set_brake(brake_input) 
