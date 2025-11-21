class_name LCWindowsManager
extends Node

var MainMenu: Window
var ChatWindow: Window
var TutorialWindow: Window

const THEME = preload("res://themes/theme.tres")


func _ready():
	var MainMenuScene = load("res://core/widgets/menu/main_menu.tscn").instantiate()
	MainMenu = make_window(MainMenuScene, "Main menu")
	# Set a larger size for the main menu
	MainMenu.min_size = Vector2(480, 640)
	MainMenu.size = Vector2(480, 720)
	add_child(MainMenu)
	center_window(MainMenu)
	
	var ChatWindowScene = load("res://modules/chat/chat-ui.tscn").instantiate()
	ChatWindow = make_window(ChatWindowScene, "Chat")
	add_child(ChatWindow)
	center_window(ChatWindow)
	
	var TutorialWindowScene = load("res://core/widgets/tutorial.tscn").instantiate()
	TutorialWindow = make_window(TutorialWindowScene, "Tutorial", false)
	add_child(TutorialWindow)
	TutorialWindow.initial_position = Window.WINDOW_INITIAL_POSITION_CENTER_PRIMARY_SCREEN
	TutorialWindow.min_size = Vector2(300, 400)
	position_top_left(TutorialWindow)

static func make_window(control, title, transparent_bg = true) -> Window:
	var win = Window.new()
	win.add_child(control)
	win.title = title
	
	# Configure window properties
	win.transparent_bg = transparent_bg
	win.unresizable = false
	win.borderless = false
	win.min_size = Vector2(300, 200)
	win.auto_translate = true
	# Ensure window size starts at a reasonable size
	win.size = Vector2(400, 500)
	win.theme = THEME
	
	# Add a styled panel for the window background
	var panel = Panel.new()
	panel.show_behind_parent = true
	panel.set_anchors_preset(Control.PRESET_FULL_RECT)
	
	win.add_child(panel)
	win.move_child(panel, 0)
	
	# Setup content anchors
	if control and control is Control:
		control.set_anchors_preset(Control.PRESET_FULL_RECT)
		control.size_flags_horizontal = Control.SIZE_FILL
		control.size_flags_vertical = Control.SIZE_FILL
		control.mouse_filter = Control.MOUSE_FILTER_STOP  # Ensure components receive input
	
	win.visible = false
	win.close_requested.connect(win.hide)
	
	# Connect focus signals to update the window appearance

	
	return win

func center_window(window: Window):
	# Get the main window size and position
	var main_window_size = get_window().size
	var main_window_position = get_window().position
	
	# Calculate the center position relative to the main window
	var x = main_window_position.x + (main_window_size.x - window.size.x) / 2
	var y = main_window_position.y + (main_window_size.y - window.size.y) / 2
	
	# Set the window position
	window.position = Vector2i(x, y)

func position_top_left(window: Window):
	# Get the main window position
	var main_window_position = get_window().position
	
	# Add a small margin from the edges
	var margin = 20
	var x = main_window_position.x + margin
	var y = main_window_position.y + margin
	
	# Set the window position
	window.position = Vector2i(x, y)

func toggle_main_menu():
	MainMenu.visible = !MainMenu.visible
	if MainMenu.visible:
		center_window(MainMenu)
		MainMenu.grab_focus()

func toggle_chat():
	ChatWindow.visible = !ChatWindow.visible
	if ChatWindow.visible:
		center_window(ChatWindow)
		ChatWindow.grab_focus()

func show_tutorial():
	TutorialWindow.show()
	position_top_left(TutorialWindow)
	TutorialWindow.grab_focus()
	
func hide_tutorial():
	TutorialWindow.hide()
