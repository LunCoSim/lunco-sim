extends Node
class_name NFTManager
signal nft_minted(token_id: int)
signal nft_load_complete(data: Dictionary)

# Contract addresses
const NFT_CONTRACT = "0x..." # Your deployed ERC-1155 contract address
const CHAIN_ID = 84532 # Polygon mainnet

# Web3 interface
var web3_interface

func _ready():
	web3_interface = get_node("/root/Web3Interface")  # Assuming you have a global Web3 singleton

func mint_design(design_data: Dictionary) -> void:
	# Convert design data to IPFS-storable format
	var metadata = {
		"name": "LunCo Supply Chain Design",
		"description": "Supply chain design created in LunCo",
		"design_data": design_data,
		"properties": {
			"nodes": design_data.nodes.size(),
			"connections": design_data.connections.size(),
			"timestamp": Time.get_unix_time_from_system()
		}
	}
	
	# Upload to IPFS first
	var ipfs_hash = await web3_interface.upload_to_ipfs(JSON.stringify(metadata))
	
	# Prepare transaction data for minting NFT
	var tx_data = {
		"to": NFT_CONTRACT,
		"method": "mint",
		"params": [
			web3_interface.get_active_account(),  # recipient
			1,  # token amount (always 1 for unique designs)
			ipfs_hash  # token URI
		]
	}
	
	# Execute transaction
	var result = await web3_interface.send_transaction(tx_data)
	if result.success:
		emit_signal("nft_minted", result.token_id)

func load_design_from_nft(token_id: int) -> void:
	# Get token URI from contract
	var uri = await web3_interface.call_contract(NFT_CONTRACT, "uri", [token_id])
	
	# Load metadata from IPFS
	var metadata = await web3_interface.load_from_ipfs(uri)
	
	# Extract design data
	if metadata.has("design_data"):
		emit_signal("nft_load_complete", metadata.design_data) 
