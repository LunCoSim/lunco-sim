class_name LCWindowsManager
extends Node

var MainMenu: Window
var ChatWindow: Window
var TutorialWindow: Window
var initialized := false

func _ready():
	print("[LCWindows] Initializing...")
	await get_tree().process_frame
	print("[LCWindows] Creating windows...")
	_create_windows()
	initialized = true
	print("[LCWindows] Windows created and initialized")

func _create_windows():
	print("[LCWindows] Loading main menu scene...")
	var MainMenuScene = load("res://core/widgets/menu/main_menu.tscn")
	if MainMenuScene:
		MainMenu = _make_window(MainMenuScene.instantiate(), "Main menu")
		print("[LCWindows] Main menu window created: ", MainMenu != null)
	else:
		push_error("[LCWindows] Failed to load main menu scene")
	
	print("[LCWindows] Loading chat scene...")
	var ChatWindowScene = load("res://modules/chat/chat-ui.tscn")
	if ChatWindowScene:
		ChatWindow = _make_window(ChatWindowScene.instantiate(), "Chat")
		print("[LCWindows] Chat window created: ", ChatWindow != null)
	else:
		push_error("[LCWindows] Failed to load chat scene")
	
	print("[LCWindows] Loading tutorial scene...")
	var TutorialWindowScene = load("res://core/widgets/tutorial.tscn")
	if TutorialWindowScene:
		TutorialWindow = _make_window(TutorialWindowScene.instantiate(), "Tutorial")
		print("[LCWindows] Tutorial window created: ", TutorialWindow != null)
	else:
		push_error("[LCWindows] Failed to load tutorial scene")

func _make_window(control: Control, title: String) -> Window:
	print("[LCWindows] Creating window: ", title)
	var win = Window.new()
	add_child(win)
	
	# Set window properties
	win.title = title
	win.unresizable = false
	win.borderless = true
	win.transparent = true
	win.transparent_bg = true
	
	# Add the content
	win.add_child(control)
	
	# Set size based on content and convert to Vector2i
	var content_size: Vector2i = Vector2i(control.get_combined_minimum_size())
	win.min_size = content_size
	win.size = content_size
	
	# Center the window - ensure both operands are Vector2i
	var viewport_rect := get_viewport().get_visible_rect()
	var viewport_size: Vector2i = viewport_rect.size
	win.position = (viewport_size - content_size) / 2
	
	# Hide by default
	win.hide()
	print("[LCWindows] Window setup complete: ", title)
	return win

func toggle_main_menu():
	print("[LCWindows] Toggling main menu...")
	if not initialized:
		push_error("[LCWindows] Windows manager not initialized")
		return
		
	if not MainMenu:
		push_error("[LCWindows] Main menu not initialized")
		return
		
	if MainMenu.visible:
		print("[LCWindows] Hiding main menu")
		MainMenu.hide()
	else:
		print("[LCWindows] Showing main menu")
		var viewport_size: Vector2i = get_viewport().get_visible_rect().size
		var win_size: Vector2i = Vector2i(MainMenu.size)
		MainMenu.position = (viewport_size - win_size) / 2
		MainMenu.show()

func toggle_chat():
	print("[LCWindows] Toggling chat...")
	if not initialized:
		push_error("[LCWindows] Windows manager not initialized")
		return
		
	if not ChatWindow:
		push_error("[LCWindows] Chat window not initialized")
		return
		
	if ChatWindow.visible:
		print("[LCWindows] Hiding chat")
		ChatWindow.hide()
	else:
		print("[LCWindows] Showing chat")
		var viewport_size: Vector2i = get_viewport().get_visible_rect().size
		var win_size: Vector2i = Vector2i(ChatWindow.size)
		ChatWindow.position = (viewport_size - win_size) / 2
		ChatWindow.show()

func show_tutoril():
	print("[LCWindows] Showing tutorial...")
	if not initialized:
		push_error("[LCWindows] Windows manager not initialized")
		return
		
	if TutorialWindow:
		var viewport_size: Vector2i = get_viewport().get_visible_rect().size
		var win_size: Vector2i = Vector2i(TutorialWindow.size)
		TutorialWindow.position = (viewport_size - win_size) / 2
		TutorialWindow.show()
	else:
		push_error("[LCWindows] Tutorial window not initialized")
	
func hide_tutorial():
	print("[LCWindows] Hiding tutorial...")
	if not initialized:
		push_error("[LCWindows] Windows manager not initialized")
		return
		
	if TutorialWindow:
		TutorialWindow.hide()
	else:
		push_error("[LCWindows] Tutorial window not initialized")

func _input(event: InputEvent):
	if event.is_action_pressed("main_menu"):
		print("[LCWindows] Main menu key detected directly in LCWindows")
		toggle_main_menu()
