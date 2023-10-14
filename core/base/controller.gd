class_name LCController
extends LCSpaceSystem

var controller_authority_id := -1

signal requesting_controller_authority(_controller);

@rpc("any_peer", "call_local")
func set_authority(owner):
	get_parent().set_multiplayer_authority(owner)

@rpc("any_peer", "call_local")
func set_controller_authority_id(_controller_authority_id):
	controller_authority_id = _controller_authority_id

@rpc("any_peer", "call_local")
func request_controlle_authority():
	requesting_controller_authority.emit(self)
	#controller_authority_id = _controller_authority_id

#--------------------------------------------
## 
static func find_controller(entity: Node) -> LCController:
	var target: LCController
	
	for N in entity.get_children():
		if N is LCController:
			target = N
			
	return target
