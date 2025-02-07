# Supply Chain Modeling Module

A Godot 4.4+ module for creating, visualizing, and simulating high-level resource and supply chain models with Web3 integration.

## Dependencies

### Core Dependencies
- Godot 4.4+
- Web3 support (optional)

### Optional Singletons
The module can use the following singletons if available in the main project:
- `Web3Interface` - For Web3/NFT functionality
- `Utils` - For utility functions

If these singletons are not available, the module will use its internal implementations.

## Directory Structure

```
supply_chain_modeling/
├── simulation/         # Simulation logic and resources
├── ui/                # UI components and widgets
├── contracts/         # Web3 smart contracts
├── singletons/        # Internal singleton implementations
└── rsct.*            # Main scene and script files
```

## Integration

1. Add the module to your project's `modules/` directory
2. Load the scene via:
   ```gdscript
   get_tree().change_scene_to_file("res://modules/supply_chain_modeling/rsct.tscn")
   ```

## Features

See `SPECIFICATION.MD` for detailed feature list and technical requirements.
