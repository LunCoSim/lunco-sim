extends Node

onready var player = $Player

func _process(_delta):
	
	# TODO: this check must be done using timer e.g. check at rate of 10 hz/sec
	# Fade out to black if falling out of the map. -17 is lower than
	# the lowest valid position on the map (which is a bit under -16).
	# At 15 units below -17 (so -32), the screen turns fully black.
		
	if player:
		if player.transform.origin.y < -17:
	#		color_rect.modulate.a = min((-17 - player.transform.origin.y) / 15, 1)
			# If we're below -40, respawn (teleport to the initial position).
			if player.transform.origin.y < -40:
	#			color_rect.modulate.a = 0
				player.transform.origin = player.initial_position

func _on_Terrain_clicked(position):
	print("Map clicked: ", position)
	var scene = load("res://addons/lunco-content/spacex-starship/source/SpaceX_Starship.fbx")
	var instance = scene.instance()
#	print("Map clicked: ", position)
	
	instance.translation = position
	add_child(instance)

#	var scene = load("res://addons/lunco-content/spacex-starship/source/SpaceX_Starship.fbx")
#	var instance = scene.instance()
#
#	instance.translation = position
#	$Map.add_child(instance)
