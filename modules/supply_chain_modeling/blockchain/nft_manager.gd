extends Node

class_name NFTManager

signal nft_minted(token_id: int)
signal nft_load_complete(data: Dictionary)

# Contract addresses
const NFT_CONTRACT = "0xec649BFc37Ec6eeae914CeDFA450FE4869487865" # Deplyed test ERC-1155 contract address
const CHAIN_ID = 84532 # Base Sepolia Testnet

# Web3 interface
var web3_interface

func _ready():
	pass
	# web3_interface = get_node("/root/Web3Interface")  # Assuming you have a global Web3 singleton

func mint_design(design_data: Dictionary) -> void:
	# Convert design data to compressed string format
	# For testing, we'll use a simplified version of the data
	var simplified_data = {
		"n": design_data.nodes,  # nodes
		"c": design_data.connections  # connections
	}
	
	# Convert to string
	var graph_string = JSON.stringify(simplified_data)
	var base64_data = Marshalls.utf8_to_base64(graph_string)

	# Prepare transaction data
	var tx_data = {
		"to": NFT_CONTRACT,
		"method": "mint",
		"params": [base64_data]
	}
	
	print(base64_data)
	# Execute transaction
	# var result = await web3_interface.send_transaction(tx_data)
	var result = {"success": true, "token_id": 1}
	if result.success:
		emit_signal("nft_minted", result.token_id)

func load_design_from_nft(token_id: int) -> void:
	# Get graph data directly from contract
	# var base64_string = await web3_interface.call_contract(
	# 	NFT_CONTRACT, 
	# 	"getGraphData",
	# 	[token_id]
	# )
	
	var base64_string = "eyJjIjpbWyJSZXNvdXJjZV9PMiIsMCwiT2JqZWN0X0ZhY3RvcnkiLDBdLFsiUmVzb3VyY2VfSDIiLDAsIk9iamVjdF9GYWN0b3J5IiwxXSxbIlNvbGFyUG93ZXJQbGFudCIsMCwiT2JqZWN0X0ZhY3RvcnkiLDJdLFsiT2JqZWN0X0ZhY3RvcnkiLDAsIlN0b3JhZ2UiLDBdXSwibiI6eyJPYmplY3RfRmFjdG9yeSI6eyJwb3MiOls1ODAuMCwxNDAuMF0sInR5cGUiOiJvYmplY3RfZmFjdG9yeS50c2NuIn0sIlJlc291cmNlX0gyIjp7InBvcyI6WzYwLjAsMjIwLjBdLCJ0eXBlIjoicmVzb3VyY2VfaDIudHNjbiJ9LCJSZXNvdXJjZV9PMiI6eyJwb3MiOls2MC4wLDAuMF0sInR5cGUiOiJyZXNvdXJjZV9vMi50c2NuIn0sIlNvbGFyUG93ZXJQbGFudCI6eyJwb3MiOls2MC4wLDQ0MC4wXSwidHlwZSI6InNvbGFyX3Bvd2VyX3BsYW50LnRzY24ifSwiU3RvcmFnZSI6eyJwb3MiOlsxMDIwLjAsMjAwLjBdLCJ0eXBlIjoic3RvcmFnZS50c2NuIn19fQ=="
	# Parse the data
	var graph_string = Marshalls.base64_to_utf8(base64_string)
	var parsed_data = JSON.parse_string(graph_string)
	
	# Convert back to full format
	var design_data = {
		"nodes": parsed_data.n,
		"connections": parsed_data.c
	}
	
	emit_signal("nft_load_complete", design_data)
