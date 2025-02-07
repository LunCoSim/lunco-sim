# LunCo Content Manager

A simple Godot plugin for managing large binary files in your project without using Git LFS.

## Overview

LunCo Content Manager allows you to:
- Store large binary files (textures, models, etc.) externally
- Download them on demand
- Keep your Git repository small
- Track binary files metadata separately

## Installation

1. Copy the `lunco-content` folder to your project's `addons` directory
2. Enable the plugin in Project -> Project Settings -> Plugins
3. You should see a "Content" button in the editor toolbar

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