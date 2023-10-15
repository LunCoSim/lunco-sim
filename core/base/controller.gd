class_name LCController
extends LCSpaceSystem

var controller_authority_id := -1

signal requesting_controller_authority(_controller, owner)
signal releasing_controller_authority(_controller)

@rpc("any_peer", "call_local")
func set_authority(owner):
	print("Setting authority ", multiplayer.get_remote_sender_id()  )
	print("Setting authority owner ", owner )
	get_parent().set_multiplayer_authority(owner)

@rpc("any_peer", "call_local")
func set_controller_authority_id(_controller_authority_id):
	print("set_controller_authority_id")
	controller_authority_id = _controller_authority_id

@rpc("any_peer", "call_local")
func request_controller_authority():
	print('request_controlle_authority')
	requesting_controller_authority.emit(self, multiplayer.get_remote_sender_id())
	#controller_authority_id = _controller_authority_id

@rpc("any_peer", "call_local")
func release_controller_authority():
	print("request_controlle_authority")
	releasing_controller_authority.emit(self)
	#controller_authority_id = _controller_authority_id


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
