## This class is responsible for managing connected users
class_name LCUsersManager
extends Node

signal users_updated()
# Every users shold be assosiated with peer (actually several peers?)
# One peer 

var users: = {} # peer_id: Profile


#----------------------------

func _on_user_connected(id, username, wallet):
	var profile: = LCProfile.new()
	
	profile.username = username
	profile.wallet = wallet
	
	users[id] = profile
	
	users_updated.emit()
	
func _on_user_disconnected(id):
	users.erase(id)
	users_updated.emit()
