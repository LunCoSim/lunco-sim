# ModelicaNFT Smart Contract

This smart contract enables the creation, management, and trading of Modelica models as NFTs on the blockchain.

## Features

- Create and mint Modelica models as NFTs
- Version control system for model updates
- Dependency tracking
- Model verification system
- License management
- Creator attribution and tracking

## Contract Structure

### Core Components

1. **ModelMetadata**
   - Model name and description
   - Model code (stored as IPFS hash)
   - Dependencies
   - Version information
   - License details
   - Creator information
   - Verification status
   - Custom parameters

2. **Version Control**
   - Track multiple versions of each model
   - Store version history
   - Maintain timestamps

### Main Functions

1. **createModel**
   - Create new model NFT
   - Set initial metadata
   - Mint token to creator

2. **updateModel**
   - Update existing model
   - Create new version
   - Maintain version history

3. **getModel**
   - Retrieve model metadata
   - Access model code
   - View version information

4. **verifyModel**
   - Mark models as verified
   - Only callable by contract owner

## Setup

1. Install dependencies:
   ```bash
   npm install @openzeppelin/contracts
   ```

2. Deploy contract:
   ```bash
   npx hardhat run scripts/deploy.js --network base-sepolia
   ```

3. Verify contract:
   ```bash
   npx hardhat verify --network base-sepolia DEPLOYED_CONTRACT_ADDRESS
   ```

## Usage

### Creating a Model

```javascript
const modelicaNFT = await ModelicaNFT.deployed();

await modelicaNFT.createModel(
    "Model Name",
    "Model Description",
    "ipfs://QmHash...",  // IPFS hash of model code
    "MIT",               // License
    []                   // Dependencies
);
```

### Updating a Model

```javascript
await modelicaNFT.updateModel(
    tokenId,
    "ipfs://QmNewHash..."  // New IPFS hash
);
```

### Getting Model Information

```javascript
const model = await modelicaNFT.getModel(tokenId);
console.log(model.name);
console.log(model.modelCode);
```

## Security Considerations

1. Model code is stored on IPFS for decentralization
2. Only creators can update their models
3. Contract can be paused in emergencies
4. Owner verification system for quality control

## License

MIT License 