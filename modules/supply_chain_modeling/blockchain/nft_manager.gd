extends Node

class_name NFTManager

signal nft_minted(token_id: int)
signal nft_load_complete(data: Dictionary)

# Contract addresses
const NFT_CONTRACT = "0xec649BFc37Ec6eeae914CeDFA450FE4869487865" # Deplyed test ERC-1155 contract address
const CHAIN_ID = 84532 # Base Sepolia Testnet

# Web3 interface
var web3_interface

# Add this constant at the top of the file
const RESOURCE_PATHS = {
	"object_factory.tscn": "res://facilities/object_factory.tscn",
	"solar_power_plant.tscn": "res://facilities/solar_power_plant.tscn",
	"storage.tscn": "res://facilities/storage.tscn",
	"resource_o2.tscn": "res://resources/resource_o2.tscn",
	"resource_h2.tscn": "res://resources/resource_h2.tscn",
	"resource_h2o.tscn": "res://resources/resource_h2o.tscn"
}

func _ready():
	web3_interface = get_node("/root/Web3Interface")
	web3_interface.connect("transaction_completed", _on_transaction_completed)

func mint_design(design_data: Dictionary) -> void:
	# Convert design data to compressed string format
	var simplified_data = {
		"n": design_data.nodes,  # nodes
		"c": design_data.connections  # connections
	}
	
	# Convert to string
	var graph_string = JSON.stringify(simplified_data)
	var base64_data = Marshalls.utf8_to_base64(graph_string)

	# Convert string to hex format
	var hex_string = string_to_hex(base64_data)
	
	# Add 0x prefix
	var prefixed_hex = "0x" + hex_string
	
	# Call mint through Web3 interface
	web3_interface.mint_blueprint(prefixed_hex, NFT_CONTRACT)

# Add this helper function to convert string to hex
func string_to_hex(input: String) -> String:
	var hex = ""
	for i in range(input.length()):
		var byte = input.unicode_at(i)
		hex += "%02x" % byte
	return hex
	
func hex_to_string(hex: String) -> String:
	var result = ""
	# Remove 0x prefix if present
	if hex.begins_with("0x"):
		hex = hex.substr(2)
	
	for i in range(0, hex.length(), 2):
		var byte = hex.substr(i, 2).hex_to_int()
		result += char(byte)
	return result


func load_design_from_nft(token_id: int) -> void:
	#var base64_string = await web3_interface.call_contract(
		#NFT_CONTRACT, 
		#"getGraphData",
		#[token_id]
	#)
	
	# var hex_string = hex_to_string(hex_string)

	var base64_string = "eyJjIjpbWyJSZXNvdXJjZV9PMiIsMCwiT2JqZWN0X0ZhY3RvcnkiLDBdLFsiUmVzb3VyY2VfSDIiLDAsIk9iamVjdF9GYWN0b3J5IiwxXSxbIlNvbGFyUG93ZXJQbGFudCIsMCwiT2JqZWN0X0ZhY3RvcnkiLDJdLFsiT2JqZWN0X0ZhY3RvcnkiLDAsIlN0b3JhZ2UiLDBdXSwibiI6eyJPYmplY3RfRmFjdG9yeSI6eyJwb3MiOls1ODAuMCwxNDAuMF0sInR5cGUiOiJvYmplY3RfZmFjdG9yeS50c2NuIn0sIlJlc291cmNlX0gyIjp7InBvcyI6WzYwLjAsMjIwLjBdLCJ0eXBlIjoicmVzb3VyY2VfaDIudHNjbiJ9LCJSZXNvdXJjZV9PMiI6eyJwb3MiOls2MC4wLDAuMF0sInR5cGUiOiJyZXNvdXJjZV9vMi50c2NuIn0sIlNvbGFyUG93ZXJQbGFudCI6eyJwb3MiOls2MC4wLDQ0MC4wXSwidHlwZSI6InNvbGFyX3Bvd2VyX3BsYW50LnRzY24ifSwiU3RvcmFnZSI6eyJwb3MiOlsxMDIwLjAsMjAwLjBdLCJ0eXBlIjoic3RvcmFnZS50c2NuIn19fQ=="

	# Decode base64 and parse the data
	var graph_string = Marshalls.base64_to_utf8(base64_string)
	
	var parsed_data = JSON.parse_string(graph_string)
	
	# Convert back to full format, adding full paths to node types
	var nodes_with_paths = {}
	for node_key in parsed_data.n.keys():
		var node_data = parsed_data.n[node_key].duplicate()
		# Get the full path from our mapping
		if RESOURCE_PATHS.has(node_data.type):
			node_data.type = RESOURCE_PATHS[node_data.type]
		else:
			push_warning("Unknown resource type: " + node_data.type)
		nodes_with_paths[node_key] = node_data
	
	var design_data = {
		"name": parsed_data.get("name", "Unnamed Blueprint"),
		"description": parsed_data.get("description", ""),
		"nodes": nodes_with_paths,
		"connections": parsed_data.c
	}
	
	emit_signal("nft_load_complete", design_data)

func _on_transaction_completed(success: bool, data: Dictionary):
	if success:
		emit_signal("nft_minted", data.hash)
