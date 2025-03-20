# Supply Chain Modeling in 3D

This module integrates the 2D Supply Chain Modeling application into the 3D world simulation.

## How It Works

1. **SubViewport Rendering**:
   - The 2D Supply Chain scene is loaded into a SubViewport
   - The SubViewport's texture is applied to a 3D quad mesh
   - This allows the 2D UI to be visible in the 3D world

2. **Input Handling**:
   - An Area3D with collision shape detects mouse interaction
   - 3D mouse coordinates are converted to 2D viewport coordinates
   - The converted input events are forwarded to the SubViewport

3. **Path Resolution**:
   - A proxy node named "RSCT" is added to the root
   - This proxy forwards method calls and property access to the actual scene
   - This solves issues with absolute paths in the original application scripts

## Usage

1. Add the `SupplyChainDisplay` scene to your 3D world
2. Press the Tab key to toggle the display on/off
3. Click and interact with the UI directly in 3D space

## Troubleshooting

If you see errors like `Node not found: "RSCT" (relative to "/root")`:
- Ensure the proxy script is correctly attached to the root node
- Check that the supply chain display is added to the "supply_chain_display" group
- Verify the scene is properly instantiated in the SubViewport

## Technical Details

The system uses three key components:
1. **SubViewport** - Renders the 2D scene to a texture
2. **Proxy Script** - Forwards method calls from absolute paths to the actual scene
3. **Input Conversion** - Maps 3D mouse coordinates to 2D UI coordinates

## Known Limitations

- Keyboard input must be captured globally (not position-based like mouse input)
- The 2D UI scale is fixed at design time and cannot be dynamically resized 