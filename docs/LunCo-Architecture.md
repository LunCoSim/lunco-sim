# LunCo Architecture

## Table of content

- [Design Principles](#design-principles)
- [Why Godot?](#why-godot)
- [Why is LunCo easily extensible?](#why-is-lunco-easily-extensible)
- [How is the Godot Plugins folder structured for convinient development?](#how-is-the-godot-plugins-folder-structured-for-convinient-development)
- [How to deal with development/download urls?](#how-to-deal-with-developmentdownload-urls)
- [What addons are used?](#what-addons-are-used)
- [Folder structure](#folder-structure)

## Design Principles

LunCo is designed with the following core principles:
- Be as open as possible (e.g. MIT or similar licence)
- Reuse existing solutions and widely adopted open standards
- Be easily extensibile
- UX is the key

## Why Godot?

- Godot is the only open AAA decent game engine
- Almost every engineering task involves 3D, 2D, and/or UI tasks so it's great to use game engine as a basis
- Godot is very lightweight with small codebase, and can even run on a Raspberry Pi 4
- Godot is quite mature. It has "flight heritage."
- It is easy to add or change core functionality, e.g. adding a custom robotic-specific physics engine


## Why is LunCo easily extensible?

1. Godot is a cross-platform engine available on most platforms
2. LunCo relies on git submodules to get latest plugins, or manual copy if a plugin's repo folder structure is inappropriate

## How is the Godot Plugins folder structured for convinient development?

1. Identify functionality that coulde be moved to a separate plugin, in terms of Godot - moved to `res://addons/{your_addon_name}`
2. Try to make the addon self-dependent
3. Create a separate git repo
4. Put your addon into the root directory of the repo
5. Add addon as a **git submodule** using:
```bash
git submodule add {url_to_repo} ./addons/{your_addon_name}
```

#### How to deal with development/download urls?

GitHub allows repositories to be downloaded via HTTPS or SSH. 
- SSH is a preferable option for development and security reasons, but it requires SSH to be set up on the host computer.
- HTTPS can be used for easier downloads.

To start developing, you'll have to do several manual steps (later a script will be added to do it automatically):
1. Git allows to use different urls for push and pull according to [the article](https://stackoverflow.com/questions/31747072/will-remote-url-for-fetch-and-push-be-different)
2. So you'll have to add to ".git/config" a push url with the right link
3. Same should be done for every submodule in ".git/modules"
4. Check ".gitmodules" file in the root folder for reference

## What addons are used?

| **Name**         | **Description**                                                                                       | **Distribution** |
| ---------------- | ----------------------------------------------------------------------------------------------------- | ---------------- |
| beehave          | Behaviour Tree for AI                                                                                 | Manuel           |
| free-look-camera | Simple free look camera                                                                               | Git              |
| imjp94.yafsm     | State machine                                                                                         | Manuel           |
| lunco-assets     | Binare assets like icons, spash for LunCo                                                             | Git              |
| lunco-cameras    | Plugin with several different cameras made for LunCo                                                  | Git              |
| lunco-content    | Repo with heavy-weight binary models, e.g. terrain. Later to be switched to IPFS content distribution | Git              |
| object-inspector | Node that helps inspects objects properties at runtime                                                | Git              |
| panku_console    | Swiss kniff for runtime debuggin. Console that allows control all the aspecs of the world in runtime  | Manuel           |                 |                                                                                                       |                  |

## Folder structure

1. `applications` - contains applications, high-level stuff build based on the core, later could be in a seperate repo
2. `addons` - folder for plugins, according to Godot suggestions
3. `core` - core lunco code


## Scene structure

 Simulation
	 -- Universe: Contains all bodies that has to be simulated
	 -- UI: Contains all the windows
	 -- SimManager: Current state of the simulation
	 -- Avatar?


## Notes

Conceptually everything starts with a Blank Simulation, Main Menu is just an Overlay in the UI, and all the apps are just modules + a specific configuration of the simulation