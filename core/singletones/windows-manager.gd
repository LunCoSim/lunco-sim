class_name LCWindowsManager
extends Node

var MainMenu: Window
var ChatWindow: Window
var TutorialWindow: Window
var CommandWindow: Window

const THEME = preload("res://themes/theme.tres")


func _ready():
	print("[LCWindows] Initializing...")
	# Setup debounce timer first so it's ready for any signals
	debounce_timer.one_shot = true
	debounce_timer.wait_time = 0.5
	debounce_timer.timeout.connect(save_window_state)
	add_child(debounce_timer)

	# Load window state immediately on startup
	load_window_state()
	
	_setup_system_windows()
	_setup_tutorial_window()
	_setup_command_window()
	
	# Connect to main window signals for changes - Do this LAST to avoid catching setup changes
	var win = get_window()
	win.size_changed.connect(_on_window_changed)

func _setup_system_windows():
	var MainMenuScene = load("res://core/widgets/menu/main_menu.tscn")
	if MainMenuScene:
		MainMenu = make_window(MainMenuScene.instantiate(), "Main menu")
		MainMenu.min_size = Vector2(480, 640)
		MainMenu.size = Vector2(480, 720)
		add_child(MainMenu)
		center_window(MainMenu)
	
	var ChatWindowScene = load("res://modules/chat/chat-ui.tscn")
	if ChatWindowScene:
		ChatWindow = make_window(ChatWindowScene.instantiate(), "Chat")
		add_child(ChatWindow)
		center_window(ChatWindow)

func _setup_tutorial_window():
	var TutorialRes = load("res://core/widgets/tutorial.tscn")
	if TutorialRes:
		TutorialWindow = make_window(TutorialRes.instantiate(), "Tutorial", true, false)
		add_child(TutorialWindow)
		TutorialWindow.initial_position = Window.WINDOW_INITIAL_POSITION_CENTER_PRIMARY_SCREEN
		TutorialWindow.min_size = Vector2(300, 400)
		center_window(TutorialWindow)

func _setup_command_window():
	var CommandRes = load("res://modules/command_ui/command_ui.tscn")
	if CommandRes:
		CommandWindow = make_window(CommandRes.instantiate(), "Command Dashboard")
		add_child(CommandWindow)
		CommandWindow.min_size = Vector2(900, 600)
		center_window(CommandWindow)
	else:
		push_error("[LCWindows] Failed to load command_ui.tscn!")
	# win.position_changed.connect(_on_window_changed) # Not available in standard Godot 4.x Window

func _notification(what):
	if what == NOTIFICATION_WM_CLOSE_REQUEST or what == NOTIFICATION_APPLICATION_FOCUS_OUT:
		save_window_state()

#-----------------------------------------------------------------------------
# Window Persistence
#-----------------------------------------------------------------------------

const CONFIG_PATH = "user://window_state.cfg"
const CONFIG_SECTION = "Window"
var debounce_timer = Timer.new()

func _on_window_changed():
	# Only start if timer is inside tree (safer check, though reordering _ready fixes the main issue)
	if debounce_timer.is_inside_tree():
		debounce_timer.start()

func save_window_state():
	var win = get_window()
	var config = ConfigFile.new()
	
	config.set_value(CONFIG_SECTION, "mode", win.mode)
	config.set_value(CONFIG_SECTION, "position", win.position)
	config.set_value(CONFIG_SECTION, "size", win.size)
	config.set_value(CONFIG_SECTION, "current_screen", win.current_screen)
	
	config.save(CONFIG_PATH)
	# print("Window state saved: ", win.position, win.size)

func load_window_state():
	var config = ConfigFile.new()
	var err = config.load(CONFIG_PATH)
	
	if err != OK:
		return # No saved state or error loading
		
	var win = get_window()
	
	# Load Mode
	var mode = config.get_value(CONFIG_SECTION, "mode", Window.MODE_WINDOWED)
	# Only restore if mode is valid
	if mode >= 0 and mode <= Window.MODE_EXCLUSIVE_FULLSCREEN:
		win.mode = mode
		
	# If in windowed mode, restore size and position
	if mode == Window.MODE_WINDOWED:
		var screen = config.get_value(CONFIG_SECTION, "current_screen", win.current_screen)
		win.current_screen = screen
		
		# Validation to ensure window assumes a safe position (e.g. not off-screen) could be added here
		var pos = config.get_value(CONFIG_SECTION, "position", win.position)
		var size = config.get_value(CONFIG_SECTION, "size", win.size)
		
		win.position = pos
		win.size = size


static func make_window(control, title, transparent_bg = true, borderless = false) -> Window:
	var win = Window.new()
	win.add_child(control)
	win.title = title
	
	# Configure window properties
	win.transparent_bg = transparent_bg
	win.unresizable = false
	win.borderless = borderless
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
	center_window(TutorialWindow)
	TutorialWindow.grab_focus()
	
func hide_tutorial():
	TutorialWindow.hide()

func toggle_command_ui():
	CommandWindow.visible = !CommandWindow.visible
	if CommandWindow.visible:
		center_window(CommandWindow)
		CommandWindow.grab_focus()
