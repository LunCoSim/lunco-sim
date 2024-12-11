extends Node

signal wallet_connected(address: String)
signal wallet_disconnected
signal transaction_completed(success: bool, data: Dictionary)

var connected_address: String = ""
var is_connected: bool = false

var js_interface

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

		window.mintNFT = async function(contractAddress, data) {
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

func mint_blueprint(data: String, contract_address: String):
	if OS.has_feature("web"):
		var result = await js_interface.mintNFT(contract_address, data)
		emit_signal("transaction_completed", result.success, result)
		return result
	return {"success": false, "error": "Not running in web context"}
