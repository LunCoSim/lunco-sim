extends MenuBar

signal new_graph_requested

func _ready() -> void:
	#hack as MenuBar by default uses PopupName of the node
	#TBD Suggest to Godot team to use window title instead
	
	self.set_menu_title(0, "File")
	self.set_menu_title(1, "NFT")
	
	# Connect the "New" menu item
	%FileMenu.connect("id_pressed", _on_file_menu_pressed)

func _on_file_menu_pressed(id: int) -> void:
	match id:
		0:
			emit_signal("new_graph_requested")
