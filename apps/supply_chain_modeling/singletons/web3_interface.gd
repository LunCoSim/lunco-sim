extends Node

signal wallet_connected(address: String)
signal wallet_disconnected
signal transaction_completed(success: bool, data: Dictionary)
signal nft_minted(token_id: int)
signal nft_load_complete(data: Dictionary)

var connected_address: String = ""
var is_connected: bool = false

var js_interface

# Constants for contract interaction
const TEST_BLUEPRINT_NFT_ABI = [
	{
		"inputs": [
			{
				"internalType": "string",
				"name": "graphData",
				"type": "string"
			}
		],
		"name": "mint",
		"outputs": [
			{
				"internalType": "uint256",
				"name": "",
				"type": "uint256"
			}
		],
		"stateMutability": "nonpayable",
		"type": "function"
	}
]

# Contract addresses
const NFT_CONTRACT = "0xec649BFc37Ec6eeae914CeDFA450FE4869487865" # Deployed test ERC-1155 contract address
const CHAIN_ID = 84532 # Base Sepolia Testnet

const RESOURCE_PATHS = {
	"object_factory.tscn": "res://facilities/object_factory.tscn",
	"solar_power_plant.tscn": "res://facilities/solar_power_plant.tscn",
	"storage.tscn": "res://facilities/storage.tscn",
	"resource_o2.tscn": "res://resources/resource_o2.tscn",
	"resource_h2.tscn": "res://resources/resource_h2.tscn",
	"resource_h2o.tscn": "res://resources/resource_h2o.tscn"
}

func _ready():
	if OS.has_feature("web"):
		# Initialize JavaScript interface
		js_interface = JavaScriptBridge.get_interface("window")
		_initialize_web3()

func _initialize_web3():
	# Inject minimal Web3 code
	js_interface.eval("""
		window.connectWallet = async function() {
			if (typeof window.ethereum !== 'undefined') {
				try {
					const accounts = await window.ethereum.request({ 
						method: 'eth_requestAccounts' 
					});
					return accounts[0];
				} catch (error) {
					console.error(error);
					return null;
				}
			}
			return null;
		};

		window.test = function(arg1, arg2) {
			console.log("test", arg1, arg2)
		}

		window.encodeFunctionCall = function(function_name, param_type, param_value) {
			return window.web3.eth.abi.encodeFunctionCall({
				"name": function_name,
				"type": 'function',
				"inputs": [{
					"type": param_type,
					"name": "arg1"
				}]
			}, [param_value])
		}

		window.mintNFT = async function(contractAddress, data) {
			console.log("mintNFT", contractAddress, data)
			if (typeof window.ethereum !== 'undefined') {
				try {
					// Basic ERC1155 mint call
					const response = await window.ethereum.request({
						method: 'eth_sendTransaction',
						params: [{
							to: contractAddress,
							from: window.ethereum.selectedAddress,
							data: data
						}]
					});
					return {success: true, hash: response};
				} catch (error) {
					console.error(error);
					return {success: false, error: error.message};
				}
			}
			return {success: false, error: 'Web3 not available'};
		};
	""")

func connect_wallet() -> void:
	if is_connected:
		push_warning("Wallet already connected")
		return

	# Implement your wallet connection logic here
	# For example:
	var result = await js_interface.connectWallet()
	if result.success:
		connected_address = result.address
		is_connected = true
		emit_signal("wallet_connected", connected_address)
	else:
		push_error("Failed to connect wallet: " + str(result.error))

func disconnect_wallet() -> void:
	if !is_connected:
		push_warning("No wallet connected")
		return
		
	# Implement your wallet disconnection logic here
	connected_address = ""
	is_connected = false
	emit_signal("wallet_disconnected")

func get_connected_address() -> String:
	return connected_address

func is_wallet_connected() -> bool:
	return is_connected

# Function to mint a new blueprint NFT
func mint_blueprint(graph_data: String, contract_address: String):
	print("mint_blueprint")
	# if not is_wallet_connected():
	# 	emit_signal("transaction_completed", false, {"error": "Wallet not connected"})
	# 	return
	# print('dada')	
	# Prepare the transaction
	var tx_data = {
		"from": "0x5242c0c4E4710785AF673D02Bf4Bf7E8842a1Cd0",
		"to": contract_address,
		"data": encode_function_call(
			"mint",
			["string"],
			[graph_data]
		)
	}
	print("mint_blueprint tx_data: ", tx_data)
	var result = await js_interface.mintNFT(contract_address, tx_data["data"])

	var success = false if result.success == null else bool(result.success)

	# emit_signal("transaction_completed", success, JSON.parse_string(JSON.stringify(result)))
	emit_signal("transaction_completed", success, {})
	return result
	
	# return {"success": false, "error": "Not running in web context"}

# Helper function to encode function calls
func encode_function_call(function_name: String, param_types: Array, param_values: Array) -> String:

	print("encode_function_call")
	print("function_name", function_name)
	print("param_types", param_types)
	print("param_values", param_values)


	var encoded_params = js_interface.encodeFunctionCall(function_name, param_types[0], param_values[0])

	print("encoded_params", encoded_params)
	# Remove '0x' from encoded_params and concatenate
	return encoded_params

# Function to check if a transaction was successful
func check_transaction(tx_hash: String) -> void:
	js_interface.eval("""
		web3.eth.getTransactionReceipt(arguments[0])
		.then(function(receipt) {
			if (receipt) {
				window.godot.emit_signal('transaction_receipt', receipt.status, receipt);
			} else {
				setTimeout(function() {
					window.godot.check_transaction(arguments[0]);
				}, 2000);
			}
		});
	""", [tx_hash])

# Add these methods for string encoding/decoding
func encode_graph_data(design_data: Dictionary) -> String:
	# Convert design data to compressed string format
	var simplified_data = {
		"n": design_data.nodes,  # nodes
		"c": design_data.connections  # connections
	}
	
	# Convert to string and encode
	var graph_string = JSON.stringify(simplified_data)
	var base64_data = Marshalls.utf8_to_base64(graph_string)
	
	# Convert to hex format
	var hex_string = string_to_hex(base64_data)
	
	# Add 0x prefix
	return "0x" + hex_string

func decode_graph_data(hex_data: String) -> Dictionary:
	# Remove 0x prefix if present
	if hex_data.begins_with("0x"):
		hex_data = hex_data.substr(2)
	
	# Convert hex to string
	var base64_string = hex_to_string(hex_data)
	
	# Decode base64 and parse
	var graph_string = Marshalls.base64_to_utf8(base64_string)
	return JSON.parse_string(graph_string)

# Helper functions
func string_to_hex(input: String) -> String:
	var hex = ""
	for i in range(input.length()):
		var byte = input.unicode_at(i)
		hex += "%02x" % byte
	return hex
	
func hex_to_string(hex: String) -> String:
	var result = ""
	for i in range(0, hex.length(), 2):
		var byte = hex.substr(i, 2).hex_to_int()
		result += char(byte)
	return result

# Add these simplified methods that were previously in NFTManager
func mint_design(design_data: Dictionary) -> void:
	var encoded_data = encode_graph_data(design_data)
	mint_blueprint(encoded_data, NFT_CONTRACT)

func load_design(token_id: int) -> void:
	#var base64_string = await call_contract(
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
