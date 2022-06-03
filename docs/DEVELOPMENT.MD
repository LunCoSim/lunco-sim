# LunCo: Development Guide

## Working with git
1. Each feature in separate branch
2. When it's ready it's merged into main
3. After merge into main everyone pull's changes into their branches
4. One branch - one responsible person
5. Binary files are stored in **git lfs**
   
## Development tools
1. Godot 3.5-rc2 – game engine, IDE 
2. Blender – 3D model creation
3. GIMP – image editor
4. Inkscape – vector image editor 
5. Visual Code + git branch, Godot extension – text editor + visual git manager
6. Git – version control
7. Git LFS – storing large files
8. [Material Maker](https://github.com/RodZill4/material-maker)
9. [godot-blender-exporter](https://github.com/godotengine/godot-blender-exporter)

## Making 3D models
1. Model in Blender
2. One model – one file
3. Use glTF 2.0 file format for exchange, preferably use [godot-blender-exporter](https://github.com/godotengine/godot-blender-exporter)
4. Files should be clean, no meshs that are not related
5. Use relative file references rather than embed
6. Try to use materials from library rather than model-specific
7. Export from Blender should be automated including baking process. Spec for that TBD
8. In Blender, this can be done by adding a Triangulate modifier to your objects and making sure Apply Modifiers is checked in the export dialog.
9. By default, Blender has backface culling disabled on materials and will export materials to match how they render in Blender. This means that materials in Godot will have their cull mode set to Disabled. This can decrease performance since backfaces will be rendered, even when they are being culled by other faces. To resolve this, enable Backface Culling in Blender's Materials tab, then export the scene to glTF again.
10. 

## Programming Godot
1. Use [Static typing](https://docs.godotengine.org/en/stable/tutorials/scripting/gdscript/static_typing.html)
   

## Getting high FPS
1. Use Resources instead of Nodes where it's possible
2. Share Resources to minimize memory consumption
3. [Fixing jitter and stutter](https://docs.godotengine.org/en/stable/tutorials/rendering/jitter_stutter.html)
   1. [Physics smooth addon](https://github.com/lawnjelly/smoothing-addon)

### Using LOD
LOD – level of detail. 

LOD0 - the best quality. 

LOD means quality models used at different distances

Could be hundreds of levels

[](https://docs.godotengine.org/en/3.0/tutorials/3d/3d_performance_and_limitations.html?highlight=lod#level-of-detail-lod)

## Style guide

[Based on Godot help](https://docs.godotengine.org/en/stable/tutorials/best_practices/project_organization.html)

For consistency across projects, we recommend following these guidelines:

* Use **snake_case** for folder and file names (with the exception of C# scripts). This sidesteps case sensitivity issues that can crop up after exporting a project on Windows. C# scripts are an exception to this rule, as the convention is to name them after the class name which should be in PascalCase.

* Use **PascalCase** for node names, as this matches built-in node casing.

* In general, keep third-party resources in a top-level *addons/* folder, even if they aren't editor plugins. This makes it easier to track which files are third-party. There are some exceptions to this rule; for instance, if you use third-party game assets for a character, it makes more sense to include them within the same folder as the character scenes and scripts.

## Localization
Use [.po](https://docs.godotengine.org/en/stable/tutorials/i18n/localization_using_gettext.html#doc-localization-using-gettext) format