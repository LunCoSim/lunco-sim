extends MenuBar

signal new_graph_requested
signal save_requested
signal load_requested

signal save_to_file_requested
signal load_from_file_requested

signal save_as_nft_requested
signal load_from_nft_requested
signal load_all_nfts_requested

func _ready() -> void:
	# Set menu titles
	self.set_menu_title(0, "File")
	self.set_menu_title(1, "NFT")
	

func _on_file_menu_pressed(id: int) -> void:
	match id:
		0: # New
			emit_signal("new_graph_requested")
		2: # Save
			emit_signal("save_requested")
		3: # Load
			emit_signal("load_requested")
		5: # Save to File
			emit_signal("save_to_file_requested")
		6: # Load from File
			emit_signal("load_from_file_requested")

func _on_nft_menu_pressed(id: int) -> void:
	match id:
		0: # Save as NFT
			emit_signal("save_as_nft_requested")
		1: # Load from NFT
			emit_signal("load_from_nft_requested")
		2: # Load all NFTs
			emit_signal("load_all_nfts_requested")
