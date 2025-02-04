## This class is responsible for managing connected users
class_name LCUsersManager
extends Node

signal users_updated()
signal user_connected(id, username, wallet)
signal user_disconnected(id)
# Every users shold be assosiated with peer (actually several peers?)
# One peer 

var users: = {} # peer_id: Profile


#----------------------------

func _on_user_connected(id: int, username: String, wallet: String):
	var profile: = LCProfile.new()
	
	profile.username = username
	profile.wallet = wallet
	
	users[id] = profile
	
	users_updated.emit()
	user_connected.emit(id, username, wallet)
	
func _on_user_disconnected(id: int):
	users.erase(id)
	users_updated.emit()
	user_disconnected.emit(id)

func find_user_id_by_name(username: String) -> int:
	for id in users:
		if users[id]["username"].to_lower() == username.to_lower():
			return id
	return -1
