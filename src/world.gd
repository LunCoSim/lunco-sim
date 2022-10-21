extends Node


# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	var character: LcCharacter = $Character
	if Input.is_action_just_pressed("ui_accept"):
		character.execute_command(LcCharacter.Commands.JUMP)
	
	# Get the input direction and handle the movement/deceleration.
	# As good practice, you should replace UI actions with custom gameplay actions.
	var input_dir = Input.get_vector("ui_left", "ui_right", "ui_up", "ui_down")
	
	if input_dir.length() > 0:
		character.execute_command_args(LcCharacter.Commands.START_MOVING, input_dir)
	else:
		character.execute_command(LcCharacter.Commands.STOP_MOVING)
