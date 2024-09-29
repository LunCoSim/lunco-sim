extends Node


# Called when the node enters the scene tree for the first time.

func _ready() -> void:
	var server = HttpServer.new()
	
	server.register_router("/", Web3Router.new("res://modules/web3"))
	server.register_router("/success", Web3ResponseRouter.new())
	add_child(server)
	server.start()
