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

## Making 3D models
1. Model in Blender
2. One model – one file
3. Use Collada (DAE) file format for exchange
4. Files should be clean, no meshs that are not related
5. Use relative file references rather than embed
6. Try to use materials from library rather than model-specific
7. Export from Blender should be automated including backing process. Spec for that TBD