As a software engineer imagine windows system for LunCo based on PankuConsole Lynx WindowsManager for Godot 4.2

Basic requirements are:
1. One singleton
2. MVC framework, meaning that UI should not be embedded into entities, rather that loaded automatically based on seletect options
3. With windows that could be closed, opened and so on
4. With main menu as widgets


From practical perspective:

1. All UI Control nodes should be in a separate scene, e.g. avatar and avatar-ui
2. A UIManager or similar entitity should connect model and ui
3. No Controls in the Universe nide
