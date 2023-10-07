class_name LCController
extends LCSpaceSystem

@rpc("any_peer", "call_local")
func set_authority(owner):
	get_parent().set_multiplayer_authority(owner)
