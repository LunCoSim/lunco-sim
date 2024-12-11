# wallet_connect_button.gd
extends Button

signal wallet_connected(address: String)
signal wallet_disconnected()

var is_connected := false
var wallet_address := ""

func _ready():
    text = "Connect Wallet"
    connect("pressed", _on_button_pressed)
    
    # Style the button
    add_theme_stylebox_override("normal", get_theme_stylebox("normal", "Button"))
    add_theme_stylebox_override("hover", get_theme_stylebox("hover", "Button"))
    add_theme_stylebox_override("pressed", get_theme_stylebox("pressed", "Button"))
    
    # Set a minimum width to prevent the button from being too small
    custom_minimum_size.x = 120

func _on_button_pressed():
    if !is_connected:
        # Here you would normally integrate with actual Web3 wallet
        # For now we'll simulate a connection
        is_connected = true
        wallet_address = "0x..." # This would come from the actual wallet
        text = "Disconnect"
        emit_signal("wallet_connected", wallet_address)
    else:
        is_connected = false
        wallet_address = ""
        text = "Connect Wallet"
        emit_signal("wallet_disconnected")