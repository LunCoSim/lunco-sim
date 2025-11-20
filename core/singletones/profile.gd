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
var _on_check_profile_nft_callback

# Called when the node enters the scene tree for the first time.
func _init():
	load_profile()
	_on_wallet_connected_callback = JavaScriptBridge.create_callback(on_wallet_connected)
	_on_check_profile_nft_callback = JavaScriptBridge.create_callback(on_check_profile_nft)

#----------------------------------

func set_username(new_username):
	if username != new_username:
		username = new_username
		save_profile()
		profile_changed.emit()

func set_wallet(new_wallet):
	if wallet != new_wallet:
		wallet = new_wallet
		save_profile()
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
		check_profile_nft()
		if wallet_info is Dictionary and wallet_info.has("wallet"):
			set_wallet(wallet_info["wallet"])
			print("Wallet connected: ", wallet)
			# Automatically check for Profile NFT
			check_profile_nft()
		else:
			print("Invalid wallet info received")
	else:
		print("No wallet info received")

func check_profile_nft():
	if JavaScriptBridge.get_interface("Login"):
		JavaScriptBridge.get_interface("Login").checkProfile(wallet, _on_check_profile_nft_callback)
	else:
		print("Login interface not available")

func on_check_profile_nft(args):
	if args and args.size() > 0:
		set_has_profile(int(args[0]))
		print("Profile NFT check result: ", has_profile)
	else:
		print("No Profile NFT check result received")

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
	var config = ConfigFile.new()

	# Load data from a file.
	var err = config.load(FULLPATH)
	
	if err != OK:
		print("Failed to load profile from ", FULLPATH, ". Error code: ", err)
		# Profile file doesn't exist or failed to load, will use default values
	
	username = config.get_value(SECTION, "username", "")
	wallet = config.get_value(SECTION, "wallet", "")
	has_profile = config.get_value(SECTION, "has_profile", 0)
	is_artizen_buyer = config.get_value(SECTION, "is_artizen_buyer", false)
