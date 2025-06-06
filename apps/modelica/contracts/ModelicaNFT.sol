// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC1155/ERC1155.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/security/Pausable.sol";
import "@openzeppelin/contracts/utils/Counters.sol";
import "@openzeppelin/contracts/utils/Strings.sol";

contract ModelicaNFT is ERC1155, Ownable, Pausable {
    using Counters for Counters.Counter;
    using Strings for uint256;
    
    struct ModelMetadata {
        string name;
        string description;
        string modelCode;          // Full Modelica model code stored on-chain
        string[] dependencies;     // Array of dependency references (token IDs or external references)
        uint256 version;          // Version number
        string license;           // License type
        address creator;          // Original creator
        uint256 timestamp;        // Creation timestamp
        bool isVerified;          // Verification status
    }

    struct Version {
        uint256 tokenId;
        uint256 versionNumber;
        string modelCode;
        uint256 timestamp;
    }

    // State variables
    Counters.Counter private _tokenIds;
    mapping(uint256 => ModelMetadata) private _models;
    mapping(uint256 => Version[]) private _modelVersions;
    mapping(address => uint256[]) private _creatorModels;
    mapping(uint256 => mapping(string => string)) private _modelParameters;
    
    // Events
    event ModelCreated(uint256 indexed tokenId, address creator, string name);
    event ModelUpdated(uint256 indexed tokenId, uint256 version);
    event ModelVerified(uint256 indexed tokenId, address verifier);
    event DependencyAdded(uint256 indexed tokenId, string dependency);
    event LicenseUpdated(uint256 indexed tokenId, string license);
    event ParameterSet(uint256 indexed tokenId, string key, string value);

    // URI configuration
    string private _baseURI = "";

    constructor() ERC1155("") Ownable(msg.sender) {}

    // Core functions
    function createModel(
        string memory name,
        string memory description,
        string memory modelCode,
        string memory license,
        string[] memory dependencies
    ) public whenNotPaused returns (uint256) {
        _tokenIds.increment();
        uint256 newTokenId = _tokenIds.current();
        
        ModelMetadata storage newModel = _models[newTokenId];
        newModel.name = name;
        newModel.description = description;
        newModel.modelCode = modelCode;  // Store full model code on-chain
        newModel.license = license;
        newModel.dependencies = dependencies;
        newModel.creator = msg.sender;
        newModel.version = 1;
        newModel.timestamp = block.timestamp;
        newModel.isVerified = false;

        _mint(msg.sender, newTokenId, 1, "");
        _creatorModels[msg.sender].push(newTokenId);
        
        Version memory initialVersion = Version({
            tokenId: newTokenId,
            versionNumber: 1,
            modelCode: modelCode,
            timestamp: block.timestamp
        });
        _modelVersions[newTokenId].push(initialVersion);

        emit ModelCreated(newTokenId, msg.sender, name);
        return newTokenId;
    }

    // Version management
    function updateModel(
        uint256 tokenId,
        string memory newModelCode
    ) public whenNotPaused {
        require(_exists(tokenId), "Model does not exist");
        require(msg.sender == _models[tokenId].creator, "Not the creator");
        
        ModelMetadata storage model = _models[tokenId];
        model.version += 1;
        model.modelCode = newModelCode;  // Update with new full model code
        model.timestamp = block.timestamp;
        
        Version memory newVersion = Version({
            tokenId: tokenId,
            versionNumber: model.version,
            modelCode: newModelCode,
            timestamp: block.timestamp
        });
        _modelVersions[tokenId].push(newVersion);

        emit ModelUpdated(tokenId, model.version);
    }

    // Parameter management
    function setModelParameter(
        uint256 tokenId,
        string memory key,
        string memory value
    ) public {
        require(_exists(tokenId), "Model does not exist");
        require(msg.sender == _models[tokenId].creator, "Not the creator");
        
        _modelParameters[tokenId][key] = value;
        emit ParameterSet(tokenId, key, value);
    }
    
    function getModelParameter(
        uint256 tokenId,
        string memory key
    ) public view returns (string memory) {
        require(_exists(tokenId), "Model does not exist");
        return _modelParameters[tokenId][key];
    }

    // Getters
    function getModel(uint256 tokenId) public view returns (
        string memory name,
        string memory description,
        string memory modelCode,
        string memory license,
        uint256 version,
        address creator,
        uint256 timestamp,
        bool isVerified
    ) {
        require(_exists(tokenId), "Model does not exist");
        ModelMetadata storage model = _models[tokenId];
        
        return (
            model.name,
            model.description,
            model.modelCode,  // Return full model code
            model.license,
            model.version,
            model.creator,
            model.timestamp,
            model.isVerified
        );
    }

    // Get just the model code (useful for large models)
    function getModelCode(uint256 tokenId) public view returns (string memory) {
        require(_exists(tokenId), "Model does not exist");
        return _models[tokenId].modelCode;
    }

    // Get specific version's model code
    function getVersionModelCode(uint256 tokenId, uint256 versionNumber) public view returns (string memory) {
        require(_exists(tokenId), "Model does not exist");
        require(versionNumber > 0 && versionNumber <= _models[tokenId].version, "Version does not exist");
        
        Version[] storage versions = _modelVersions[tokenId];
        for (uint i = 0; i < versions.length; i++) {
            if (versions[i].versionNumber == versionNumber) {
                return versions[i].modelCode;
            }
        }
        
        revert("Version not found");
    }

    function getModelDependencies(uint256 tokenId) public view returns (string[] memory) {
        require(_exists(tokenId), "Model does not exist");
        return _models[tokenId].dependencies;
    }

    function getModelVersions(uint256 tokenId) public view returns (Version[] memory) {
        require(_exists(tokenId), "Model does not exist");
        return _modelVersions[tokenId];
    }

    function getCreatorModels(address creator) public view returns (uint256[] memory) {
        return _creatorModels[creator];
    }

    // Verification system
    function verifyModel(uint256 tokenId) public onlyOwner {
        require(_exists(tokenId), "Model does not exist");
        _models[tokenId].isVerified = true;
        emit ModelVerified(tokenId, msg.sender);
    }

    // URI handling
    function setBaseURI(string memory newBaseURI) public onlyOwner {
        _baseURI = newBaseURI;
    }
    
    function uri(uint256 tokenId) public view override returns (string memory) {
        require(_exists(tokenId), "URI query for nonexistent token");
        
        return string(abi.encodePacked(
            _baseURI,
            tokenId.toString(),
            ".json"
        ));
    }

    // Utility functions
    function _exists(uint256 tokenId) internal view returns (bool) {
        return tokenId > 0 && tokenId <= _tokenIds.current();
    }

    function pause() public onlyOwner {
        _pause();
    }

    function unpause() public onlyOwner {
        _unpause();
    }
} 