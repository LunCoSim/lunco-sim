## This class represents user profile, and includes integration with web3
# TBD implementation and integration with https://github.com/LunCoSim/lc-web3
class_name LCProfile
extends Node

#----------------------------------

signal profile_changed()

#----------------------------------

@export var username: String : set = set_username
@export var wallet: String : set = set_wallet
@export var has_profile: bool : set = set_has_profile

#----------------------------------
const FILENAME = "profile.cfg"
const PATH = "user://"
const FULLPATH = PATH + FILENAME

const SECTION = "Profile"
#----------------------------------

# Called when the node enters the scene tree for the first time.
func _init():
	load_profile()

#----------------------------------

func set_username(_username):
	if username != _username:
		username = _username
		save_profile()
		profile_changed.emit()

func set_wallet(_wallet):
	if wallet != _wallet:
		wallet = _wallet
		save_profile()
		if Messenger:
			Messenger.profile_wallet_changed.emit()
		profile_changed.emit()

func set_has_profile(_has_profile):
	if has_profile != _has_profile:
		has_profile = _has_profile
		#save_profile()
		#if Messenger:
			#Messenger.profile_wallet_changed.emit()
		#profile_changed.emit()
		
#----------------------------------
func login():
	pass
	
func on_login_success():
	pass
	
func get_perks():
	pass
	
func on_perks_loaded():
	pass

func is_donor()->bool:
	return false
	
func is_special_donor()->bool:
	return false

#-----------------------------
func save_profile():
	# Create new ConfigFile object.
	var config = ConfigFile.new()

	# Store some values.
	config.set_value(SECTION, "username", username)
	config.set_value(SECTION, "wallet", wallet)

	# Save it to a file (overwrite if already exists).
	config.save(FULLPATH)

func load_profile():
	var score_data = {}
	var config = ConfigFile.new()

	# Load data from a file.
	var err = config.load(FULLPATH)

	# If the file didn't load, ignore it.
	if err != OK:
		return
	
	username = config.get_value(SECTION, "username", "")
	wallet = config.get_value(SECTION, "wallet", "")
	
