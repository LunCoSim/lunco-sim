extends Node


# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	var character: LcCharacter = $Character
	
	var camera: LcSpringArmCamera = $Camera
	
	if Input.is_action_just_pressed("ui_accept"):
		character.execute_command(LcCharacter.Commands.JUMP)
	
	# Get the input direction and handle the movement/deceleration.
	# As good practice, you should replace UI actions with custom gameplay actions.
	var input_dir = Input.get_vector("ui_left", "ui_right", "ui_up", "ui_down")
	
	if input_dir.length() > 0:
		camera.execute_command_args(LcSpringArmCamera.Commands.START_ROTATION, input_dir)
	else:
		camera.execute_command(LcSpringArmCamera.Commands.STOP_ROTATION)
	
	
	# Get the input direction and handle the movement/deceleration.
	# As good practice, you should replace UI actions with custom gameplay actions.
	var move_dir = Input.get_vector("move_left", "move_right", "move_forward", "move_backwards")
	
	if move_dir.length() > 0:
		character.execute_command_args(LcCharacter.Commands.START_MOVING, move_dir)
	else:
		character.execute_command(LcCharacter.Commands.STOP_MOVING)
