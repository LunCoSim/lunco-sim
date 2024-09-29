## This class represents user profile, and includes integration with web3
# TBD implementation and integration with https://github.com/LunCoSim/lc-web3
class_name LCProfile
extends Node

#----------------------------------

signal profile_changed()

#----------------------------------

@export var username: String : set = set_username
@export var wallet: String : set = set_wallet
@export var has_profile: int : set = set_has_profile
@export var is_artizen_buyer: bool : set = set_is_artizen_buyer

const FILENAME = "profile.cfg"
const PATH = "user://"
const FULLPATH = PATH + FILENAME

const SECTION = "Profile"
#----------------------------------

var _on_wallet_connected_callback

# Called when the node enters the scene tree for the first time.
func _init():
	load_profile()
	_on_wallet_connected_callback = JavaScriptBridge.create_callback(on_wallet_connected)

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
		
func set_is_artizen_buyer(_is_artizen_buyer):
	if is_artizen_buyer != _is_artizen_buyer:
		is_artizen_buyer = _is_artizen_buyer
		save_profile()
		profile_changed.emit()

#----------------------------------
func login():
	if JavaScriptBridge.get_interface("Login"):
		JavaScriptBridge.get_interface("Login").login(_on_wallet_connected_callback)
	else:
		print("Login interface not available")

func on_wallet_connected(args):
	print("on_wallet_connected: ", args)
	if args and args.size() > 0:
		var wallet_info = args[0]
		set_wallet(wallet_info["wallet"])
		print("Wallet connected: ", wallet)
		if wallet_info is Dictionary and wallet_info.has("wallet"):
			set_wallet(wallet_info["wallet"])
			print("Wallet connected: ", wallet)
			# You might want to fetch additional info here, like has_profile or is_artizen_buyer
		else:
			print("Invalid wallet info received")
	else:
		print("No wallet info received")

func logout():
	set_wallet("")
	set_has_profile(0)
	set_is_artizen_buyer(false)
	profile_changed.emit()

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
	config.set_value(SECTION, "has_profile", has_profile)
	config.set_value(SECTION, "is_artizen_buyer", is_artizen_buyer)

	# Save it to a file (overwrite if already exists).
	config.save(FULLPATH)

func load_profile():
	var score_data = {}
	var config = ConfigFile.new()

	# Load data from a file.
	var err = config.load(FULLPATH)

	# If the file didn't load, ignore it.
	if err != OK:
		print("Error loading profile")
		username = "unknown"
		wallet = "unknown"
		return
	
	username = config.get_value(SECTION, "username", "")
	wallet = config.get_value(SECTION, "wallet", "")
	has_profile = config.get_value(SECTION, "has_profile", 0)
	is_artizen_buyer = config.get_value(SECTION, "is_artizen_buyer", false)
