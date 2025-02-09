class_name LCWindowsManager
extends Node

var MainMenu: Window
var ChatWindow: Window
var TutorialWindow: Window


func _ready():
	var MainMenuScene = load("res://core/widgets/menu/main_menu.tscn").instantiate()
	MainMenu = make_window(MainMenuScene, "Main menu")
	
	var ChatWindowScene = load("res://modules/chat/chat-ui.tscn").instantiate()
	ChatWindow = make_window(ChatWindowScene, "Chat")
	
	var TutorialWindowScene = load("res://core/widgets/tutorial.tscn").instantiate()
	TutorialWindow = make_window(TutorialWindowScene, "Tutorial")
	
static func make_window(control, title) -> Window:
	var win = Window.new()
	win.add_child(control)
	win.title = title
	win.size = control.get_size()
	win.visible = false
	win.unresizable = true
	win.close_requested.connect(win.hide)
	return win

func toggle_main_menu():
	MainMenu.visible = !MainMenu.visible

func toggle_chat():
	ChatWindow.visible = !ChatWindow.visible

func show_tutorial():
	TutorialWindow.show()
	
func hide_tutorial():
	TutorialWindow.hide()
