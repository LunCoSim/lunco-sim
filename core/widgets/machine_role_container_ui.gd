extends VBoxContainer

# Called when the node enters the scene tree for the first time.
func _ready():
	if multiplayer.is_server():
		%MachineRole.text = "Server"
	else:
		%MachineRole.text = "Peer id: " + str(multiplayer.get_unique_id())
