// SPDX-License-Identifier: MIT
// POC for NFT Blueprints contract

pragma solidity ^0.8.0;

import "@openzeppelin/contracts/token/ERC1155/ERC1155.sol";
import "@openzeppelin/contracts/utils/Counters.sol";

contract TestBlueprintNFT is ERC1155 {
    using Counters for Counters.Counter;
    Counters.Counter private _tokenIds;
    
    // Mapping from token ID to graph data
    mapping(uint256 => string) private _tokenData;
    
    constructor() ERC1155("") {}
    
    function mint(string memory graphData) public returns (uint256) {
        _tokenIds.increment();
        uint256 newTokenId = _tokenIds.current();
        
        _mint(msg.sender, newTokenId, 1, "");
        _tokenData[newTokenId] = graphData;
        
        return newTokenId;
    }
    
    function getGraphData(uint256 tokenId) public view returns (string memory) {
        require(_exists(tokenId), "Token does not exist");
        return _tokenData[tokenId];
    }
    
    function _exists(uint256 tokenId) internal view returns (bool) {
        return tokenId > 0 && tokenId <= _tokenIds.current();
    }
}
