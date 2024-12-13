extends MenuBar


func _ready() -> void:
	#hack as MenuBar by default uses PopupName of the node
	#TBD Suggest to Godot team to use window title instead
	
	self.set_menu_title(0, "File")
	self.set_menu_title(1, "NFT")
