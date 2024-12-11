# wallet_connect_button.gd
extends Button

signal wallet_connected(address: String)
signal wallet_disconnected()

var is_connected := false
var wallet_address := ""
var web3_interface

func _ready():
    text = "Connect Wallet"
    connect("pressed", _on_button_pressed)
    
    # Get Web3 interface
    web3_interface = get_node("/root/Web3Interface")
    web3_interface.connect("wallet_connected", _on_wallet_connected)
    web3_interface.connect("wallet_disconnected", _on_wallet_disconnected)
    
    # Style the button
    add_theme_stylebox_override("normal", get_theme_stylebox("normal", "Button"))
    add_theme_stylebox_override("hover", get_theme_stylebox("hover", "Button"))
    add_theme_stylebox_override("pressed", get_theme_stylebox("pressed", "Button"))
    
    # Set a minimum width to prevent the button from being too small
    custom_minimum_size.x = 120

func _on_button_pressed():
    if !is_connected:
        web3_interface.connect_wallet()
    else:
        is_connected = false
        wallet_address = ""
        text = "Connect Wallet"
        emit_signal("wallet_disconnected")

func _on_wallet_connected(address: String):
    is_connected = true
    wallet_address = address
    text = address.substr(0, 6) + "..." + address.substr(-4)
    emit_signal("wallet_connected", address)

func _on_wallet_disconnected():
    is_connected = false
    wallet_address = ""
    text = "Connect Wallet"
    emit_signal("wallet_disconnected")