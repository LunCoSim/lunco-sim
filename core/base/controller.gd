class_name LCController
extends LCSpaceSystem



@rpc("any_peer", "call_local")
func set_authority(_owner):
	print("Setting authority ", multiplayer.get_remote_sender_id()  )
	print("Setting authority owner ", _owner )
	get_parent().set_multiplayer_authority(_owner)

## Returns true if this controller or its parent has multiplayer authority.
## Provides a consistent way to check authority across all controller types.
func has_authority() -> bool:
	var parent = get_parent()
	if parent:
		return parent.is_multiplayer_authority()
	return is_multiplayer_authority()


	
#--------------------------------------------
## 
static func find_controller(entity: Node) -> LCController:
	var target: LCController
	
	if entity:
		for N in entity.get_children():
			if N is LCController:
				target = N
				break
			
	return target
