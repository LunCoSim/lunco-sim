# LunCo

LunCo is an open-source engineering software for Lunar Settlement design built with Godot 4.

*For older version check branch godot3.5*

## Vision

LunCo provides a set of opensource applications for Lunar Base engineering including:
1. Requirements Management
2. Models visualisations
3. Collaborative training
4. Digital twin of a lunar colony


## Installation

1. Install [Godot 4](https://godotengine.org/download/)
2. Install [FBX2glTF](https://github.com/godotengine/FBX2glTF/tags):
	1. Download the file
	2. Godot will ask for this file when you will open the project
3. Install [git lfs](https://github.com/git-lfs/git-lfs#getting-started)
4. Clone this repo in a terminal: 
```bash
    git clone -b main --single-branch --recurse-submodules https://github.com/LunCoSim/lunco-sim.git
```
5. After cloning, change directory to project folder
```bash
	cd lunco-sim
```

6. Run below command to install addons using [gd-plug](https://github.com/imjp94/gd-plug)
```bash
    ./install_addons.sh
```

7. Open the project in Godot and run

## Documentation

[Click here to learn how to develop with LunCo](./docs/LunCo%20Docs/LunCo-Documentation.md)

