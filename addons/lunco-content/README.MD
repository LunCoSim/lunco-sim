# LunCo Content Manager

A simple Godot plugin for managing large binary files in your project without using Git LFS.

## Overview

LunCo Content Manager allows you to:
- Store large binary files (textures, models, etc.) externally
- Download them on demand
- Keep your Git repository small
- Track binary files metadata separately

## Installation

### Method 1: Using gd-plug (Recommended)

1. Make sure you have [gd-plug](https://github.com/imjp94/gd-plug) installed
2. Add to your `plug.gd`:
```gdscript
func _plugging() -> void:
    plug("LunCoSim/lunco-content", {"include": ["."]})
```
3. Run installation:
```bash
./plug.gd install
```

### Method 2: Manual Installation

1. Clone or download this repository
2. Copy the `lunco-content` folder to your project's `addons` directory
3. Enable the plugin in Project -> Project Settings -> Plugins
4. Restart Godot Editor

### Verifying Installation

1. After restarting Godot, you should see a "Content" button in the editor toolbar
2. Click it to see available options:
   - Check Missing Content
   - Download All Content
3. If you don't see the button, check:
   - Project Settings -> Plugins -> Make sure "LunCo Content Manager" is enabled
   - Editor output for any error messages

## Usage

### Creating Content Files

For each large binary file you want to manage, create a `.content` file next to it with the same name:

```
content/
├── large_texture.png
└── large_texture.png.content
```

The `.content` file format:
```ini
[source]
url="https://your-storage.com/path/to/file.png"
size=1234567
checksum="sha256:..." # Optional

[metadata]
description="Description of the file"
author="Author Name"
license="License Type"
```

### Managing Content

1. Click the "Content" button in the toolbar
2. Use "Check Missing Content" to see what files need to be downloaded
3. Use "Download All Content" to download missing files

### Git Integration

1. Add `.content` files to Git
2. Add the actual binary files to `.gitignore`:
```
# .gitignore
*.png
*.jpg
*.jpeg
*.glb
# ... other binary formats
```

## Example

Here's a complete example of a `.content` file:
```ini
[source]
url="https://example.com/large_texture.png"
size=1048576
checksum="sha256:123..."

[metadata]
description="High resolution texture for the moon surface"
author="NASA"
license="Public Domain"
```

## Features

- [x] Automatic content scanning
- [x] On-demand downloading
- [x] Simple text-based metadata
- [x] HTTP(S) support
- [ ] IPFS support (planned)
- [ ] Progress reporting (planned)
- [ ] Checksum verification (planned)
- [ ] Upload interface (planned)

## Contributing

Feel free to submit issues and pull requests to improve the plugin.

## License

MIT License - See LICENSE file for details 