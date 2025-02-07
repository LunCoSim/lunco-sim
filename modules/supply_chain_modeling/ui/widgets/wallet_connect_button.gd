# wallet_connect_button.gd
extends Button

var web3_interface

func _ready():
	text = "Connect Wallet"
	connect("pressed", _on_button_pressed)
	
	# Get Web3 interface from the root scene
	await get_tree().create_timer(0.1).timeout  # Wait for parent scene to initialize
	var root = get_tree().root.get_node("RSCT")
	if root:
		web3_interface = root.Web3Interface
		web3_interface.connect("wallet_connected", _on_wallet_connected)
		web3_interface.connect("wallet_disconnected", _on_wallet_disconnected)
	
	# Style the button
	add_theme_stylebox_override("normal", get_theme_stylebox("normal", "Button"))
	add_theme_stylebox_override("hover", get_theme_stylebox("hover", "Button"))
	add_theme_stylebox_override("pressed", get_theme_stylebox("pressed", "Button"))
	
	custom_minimum_size.x = 120

func _on_button_pressed():
	if !web3_interface:
		return
		
	if text == "Connect Wallet":
		web3_interface.connect_wallet()
	else:
		web3_interface.disconnect_wallet()

func _on_wallet_connected(address: String) -> void:
	text = address.substr(0, 6) + "..." + address.substr(-4)

func _on_wallet_disconnected() -> void:
	text = "Connect Wallet"
