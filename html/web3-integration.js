/**
 * Web3 and MetaMask Integration for LunCo
 * Provides wallet connection, authentication, and token gating functionality
 */

// Initialize web3
let web3 = new Web3(Web3.givenProvider || "ws://localhost:8545");

// Check if MetaMask is installed
if (typeof window.ethereum !== 'undefined' || (typeof window.web3 !== 'undefined')) {
    web3 = new Web3(window.ethereum || window.web3.currentProvider);
} else {
    console.log('MetaMask not found. Install it to proceed.');
}

/**
 * Generic API call helper function
 * @param {string} endpoint - API endpoint URL
 * @param {object} data - Data to send in the request body
 * @returns {Promise<object>} Response JSON
 */
async function apiCall(endpoint, data) {
    const res = await fetch(endpoint, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(data)
    });
    return res.json();
}

/**
 * Check if a user has the required token balance
 * @param {string} account - Ethereum account address
 * @param {function} _callback - Callback function to receive the balance
 */
async function checkProfile(account, _callback) {
    console.log("checkProfile, ", account);
    const contract = new web3.eth.Contract(contractABI, tokenContract);
    const balance = await contract.methods.balanceOf(account, tokenId).call();
    console.log('balance of: ', balance);
    _callback(balance);
}

/**
 * Initiate MetaMask login and sign authentication message
 * @param {function} _callback - Callback function to receive login result
 */
async function login(_callback) {
    try {
        const accounts = await window.ethereum.request({ method: 'eth_requestAccounts' });
        const account = accounts[0];

        const message = "Sign this message to log into LunCo";
        const signature = await web3.eth.personal.sign(message, account, "");

        console.log('Logged in with account:', account);
        console.log('Signature:', signature);

        // Call /success API (currently commented out)
        // const successData = await apiCall('/success', { wallet: account, signature });
        console.log('Success API response:', { wallet: account, signature });
        _callback({ wallet: account, signature });

    } catch (error) {
        console.log('Login canceled:', error);

        // Call /cancel API (currently commented out)
        // const cancelData = await apiCall('/cancel', {});
        console.log('Cancel API response:', error);
    }
}

/**
 * Global Login interface exposed to Godot
 * Accessible via JavaScriptBridge.get_interface("Login")
 * Must be on window object for Godot to access it
 */
window.Login = {
    login: login,
    checkProfile: checkProfile
};

/**
 * Optional: Attach login to a button if it exists in the DOM
 * This allows for traditional HTML button-based login
 */
document.addEventListener('DOMContentLoaded', function () {
    var loginBtn = document.getElementById('loginBtn');
    if (loginBtn) {
        loginBtn.addEventListener('click', login);
    }
});
