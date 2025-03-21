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
	position_top_left(TutorialWindow)
	
static func make_window(control, title) -> Window:
	var win = Window.new()
	win.add_child(control)
	win.title = title
	
	# Set size with some padding
	var control_size = control.get_combined_minimum_size()
	win.size = control_size + Vector2(40, 40)  # Add padding
	
	win.visible = false
	win.unresizable = true
	win.close_requested.connect(win.hide)
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
	var margin = 10
	var x = main_window_position.x + margin
	var y = main_window_position.y + margin
	
	# Set the window position
	window.position = Vector2i(x, y)

func toggle_main_menu():
	if !MainMenu.visible:
		# Before showing, update the size based on content
		var main_menu_content = MainMenu.get_child(0)
		var content_size = main_menu_content.get_combined_minimum_size()
		MainMenu.size = content_size + Vector2(40, 40)  # Add padding
		
		MainMenu.visible = true
		
		# Center the window
		center_window(MainMenu)
		
		# Make sure the menu is visible within the window boundaries
		var window_size = get_window().size
		var menu_pos = MainMenu.position
		
		# Check if menu extends beyond window bottom
		if menu_pos.y + MainMenu.size.y > window_size.y:
			# Adjust position to fit vertically
			menu_pos.y = max(0, window_size.y - MainMenu.size.y - 20)
			MainMenu.position = menu_pos
	else:
		MainMenu.visible = false

func toggle_chat():
	ChatWindow.visible = !ChatWindow.visible
	if ChatWindow.visible:
		center_window(ChatWindow)

func show_tutorial():
	TutorialWindow.show()
	position_top_left(TutorialWindow)
	
func hide_tutorial():
	TutorialWindow.hide()
