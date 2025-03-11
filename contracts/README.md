# ModelicaNFT Smart Contract

This smart contract enables the creation, management, and trading of Modelica models as NFTs on the blockchain with **full on-chain storage** of model code.

## Features

- Create and mint Modelica models as NFTs
- **Store model code fully on-chain** (no IPFS dependency)
- Version control system for model updates
- Dependency tracking
- Model verification system
- License management
- Creator attribution and tracking
- Custom parameter storage

## On-Chain Storage Benefits

- **True decentralization** - Models exist entirely on the blockchain
- **Data permanence** - Models are preserved as long as the blockchain exists
- **No external dependencies** - No reliance on IPFS or other storage systems
- **Atomic transactions** - Model and metadata stored in a single transaction
- **Simplified architecture** - Direct access to model code

## Contract Structure

### Core Components

1. **ModelMetadata**
   - Model name and description
   - Complete model code stored on-chain
   - Dependencies
   - Version information
   - License details
   - Creator information
   - Verification status

2. **Version Control**
   - Track multiple versions of each model
   - Store complete code for each version
   - Maintain timestamps

3. **Parameter Storage**
   - Store custom key-value parameters
   - Flexible metadata extension

### Main Functions

1. **createModel**
   - Create new model NFT with full code
   - Set initial metadata
   - Mint token to creator

2. **updateModel**
   - Update existing model with new code
   - Create new version
   - Maintain version history

3. **getModel / getModelCode**
   - Retrieve model metadata
   - Access complete model code
   - View version information

4. **setModelParameter / getModelParameter**
   - Store custom parameters
   - Retrieve parameter values

5. **verifyModel**
   - Mark models as verified
   - Only callable by contract owner

## Gas Optimization Considerations

When storing large models on-chain:

1. **Model Size** - Consider breaking very large models into components
2. **Gas Limits** - Be aware of block gas limits (currently ~30M on Ethereum)
3. **Cost Efficiency** - For extremely large models, consider hybrid approaches
4. **Compression** - Use client-side compression before storing

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
    "model MyModel\n  parameter Real x = 1.0;\nequation\n  der(x) = -x;\nend MyModel;",  // Full model code
    "MIT",               // License
    []                   // Dependencies
);
```

### Updating a Model

```javascript
await modelicaNFT.updateModel(
    tokenId,
    "model MyModel\n  parameter Real x = 2.0;\nequation\n  der(x) = -x*2;\nend MyModel;"  // Updated model code
);
```

### Getting Model Information

```javascript
const model = await modelicaNFT.getModel(tokenId);
console.log(model.name);
console.log(model.modelCode);

// Or just get the code
const code = await modelicaNFT.getModelCode(tokenId);
console.log(code);
```

### Working with Parameters

```javascript
// Set custom parameters
await modelicaNFT.setModelParameter(tokenId, "simulationTime", "100");
await modelicaNFT.setModelParameter(tokenId, "solver", "dassl");

// Get parameters
const simTime = await modelicaNFT.getModelParameter(tokenId, "simulationTime");
console.log(simTime);  // "100"
```

## Security Considerations

1. Model code is stored directly on-chain for maximum security
2. Only creators can update their models
3. Contract can be paused in emergencies
4. Owner verification system for quality control

## License

MIT License 