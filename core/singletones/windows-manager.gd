class_name LCWindowsManager
extends Node

var MainMenu: Window
var ChatWindow: Window
var TutorialWindow: Window


func _ready():
	var MainMenuScene = load("res://core/widgets/menu/main_menu.tscn").instantiate()
	MainMenu = make_window(MainMenuScene, "Main menu")
	add_child(MainMenu)
	center_window(MainMenu)
	
	var ChatWindowScene = load("res://modules/chat/chat-ui.tscn").instantiate()
	ChatWindow = make_window(ChatWindowScene, "Chat")
	add_child(ChatWindow)
	center_window(ChatWindow)
	
	var TutorialWindowScene = load("res://core/widgets/tutorial.tscn").instantiate()
	TutorialWindow = make_window(TutorialWindowScene, "Tutorial")
	add_child(TutorialWindow)
	center_window(TutorialWindow)
	
static func make_window(control, title) -> Window:
	var win = Window.new()
	win.add_child(control)
	win.title = title
	win.size = control.get_size()
	win.visible = false
	win.unresizable = true
	win.close_requested.connect(win.hide)
	win.initial_position = Window.WINDOW_INITIAL_POSITION_CENTER_SCREEN_WITH_MOUSE_FOCUS
	return win

func center_window(window: Window):
	# Get the screen size
	var screen_size = DisplayServer.screen_get_size()
	# Calculate the center position
	var x = (screen_size.x - window.size.x) / 2
	var y = (screen_size.y - window.size.y) / 2
	# Set the window position
	window.position = Vector2i(x, y)

func toggle_main_menu():
	MainMenu.visible = !MainMenu.visible
	if MainMenu.visible:
		center_window(MainMenu)

func toggle_chat():
	ChatWindow.visible = !ChatWindow.visible
	if ChatWindow.visible:
		center_window(ChatWindow)

func show_tutorial():
	TutorialWindow.show()
	center_window(TutorialWindow)
	
func hide_tutorial():
	TutorialWindow.hide()
