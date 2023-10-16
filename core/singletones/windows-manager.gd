class_name LCWindowsManager
extends Node

var MainMenu: PankuLynxWindow
var ChatWindow: PankuLynxWindow


func _ready():
	var MainMenuScene = preload("res://core/widgets/menu/main_menu.tscn").instantiate()
	MainMenu = LCWindowsManager.make_window(MainMenuScene, "Main menu")
	
	var ChatWindowScene = preload("res://modules/chat/chat-ui.tscn").instantiate()
	ChatWindow = LCWindowsManager.make_window(ChatWindowScene, "Chat")
	
static func make_window(control, title):
	var win: PankuLynxWindow = Panku.windows_manager.create_window(control)
	
	var size = control.get_minimum_size() + Vector2(0, win._window_title_container.get_minimum_size().y)
	win.set_custom_minimum_size(size)
	win.size = win.get_minimum_size()

	win.set_window_title_text(title)
	win.hide_window()
	win.queue_free_on_close = false
	return win

func toggle_main_menu():
	# Workaround, buildin toggle function fails
	if MainMenu.visible:
		MainMenu.hide_window()
	else:
		MainMenu.show_window()

func toggle_chat():
	# Workaround, buildin toggle function fails
	if ChatWindow.visible:
		ChatWindow.hide_window()
	else:
		ChatWindow.show_window()
