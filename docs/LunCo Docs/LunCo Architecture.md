# LunCo Architecture

## Table of content

- [[#Design Principles|Design Principles]]
- [[#Why Godot?|Why Godot?]]
- [[#Why LunCo is easilly extandable?|Why LunCo is easilly extandable?]]
- [[#How to structure Godot Plugins folder for convinient development?|How to structure Godot Plugins folder for convinient development?]]
- [[#What addons are used?|What addons are used?]]
- [[#Folder structure|Folder structure]]

## Design Principles

Lunco is designed with core basic ideas:
- as open as possible (e.g. MIT or similar licence)
- reuse existing solutions and widely adopted open standards
- easilly extensibility 
- UX is the key

## Why Godot?

- its the only open AAA decent game engine
- almost every engineering task involves 3D, 2D, UI tasks, so it's great to use game engine as a basis
- it's very lightweigh, with small codebase, can run on raspberri pi4
- it's a quite old solution, has "flight heritage"
- easy to add or change core functionality, e.g. add custom robotic-specific physics engine


## Why LunCo is easilly extandable?

1. Godot is crossplatform engine available on most platforms
2. LunCo relies on git submodules to get latest plugins, or manual copy if plugin's repo folder structure is inappropriate

## How to structure Godot Plugins folder for convinient development?

1. Identify functionality that coulde be moved to a separate plugin, in terms of Godot - moved to "res://addons/{you_addon_name}"
2. Try to make the addon iself-dependent
3. Create a separate git repo
4. Put your addon into root of the repo
5. Add addon as **git submodule** using:

		git submodule add {url_to_repo} ./addons/{your_addon_name}

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

1. applications - contains applications, high-level stuff build based on the core, later could be in a seperate repo
2. addons - folder for plugins, according to Godot suggestions
3. core - core lunco code
