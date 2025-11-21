extends Node

# UI Helper - Contains common UI utilities and configuration
# Use this instead of hard-coding UI values for more consistency and flexibility

# Window settings
const WINDOW_MIN_WIDTH = 350
const WINDOW_DEFAULT_WIDTH = 400
const WINDOW_DEFAULT_HEIGHT = 0  # Auto height based on content

# Margins and spacing
const MARGIN_SMALL = 8
const MARGIN_MEDIUM = 16
const MARGIN_LARGE = 24
const SPACING_SMALL = 8
const SPACING_MEDIUM = 16

# Corner radii
const CORNER_RADIUS_SMALL = 4
const CORNER_RADIUS_MEDIUM = 8
const CORNER_RADIUS_LARGE = 12

# Color variants - centralize color management
const COLOR_BG = Color(0.13, 0.13, 0.13, 0.95)
const COLOR_BG_LIGHT = Color(0.18, 0.18, 0.18, 1.0)
const COLOR_BORDER = Color(0.25, 0.25, 0.25, 1.0)
const COLOR_ACCENT = Color(0.2, 0.4, 0.8, 1.0)
const COLOR_TEXT = Color(0.875, 0.875, 0.875, 1.0)

# Entity button settings
const ENTITY_BUTTON_MIN_SIZE = Vector2(90, 40)
const ENTITY_BUTTON_TEXT_MAX_LENGTH = 10

# Apply consistent window settings
func setup_window(window: Window) -> void:
	window.initial_position = Window.WINDOW_INITIAL_POSITION_CENTER_PRIMARY_SCREEN
	window.min_size = Vector2i(WINDOW_MIN_WIDTH, 0)
	window.size = Vector2i(WINDOW_DEFAULT_WIDTH, WINDOW_DEFAULT_HEIGHT)
	window.borderless = true
	window.transparent = true
	
# Apply panel styling consistently
func setup_panel(panel: PanelContainer) -> void:
	panel.theme_type_variation = "PanelContainerOverlay"

# Apply consistent container spacing
func setup_containers(vbox: VBoxContainer, margin: MarginContainer) -> void:
	margin.add_theme_constant_override("margin_left", MARGIN_MEDIUM)
	margin.add_theme_constant_override("margin_top", MARGIN_MEDIUM)
	margin.add_theme_constant_override("margin_right", MARGIN_MEDIUM)
	margin.add_theme_constant_override("margin_bottom", MARGIN_MEDIUM)
	vbox.add_theme_constant_override("separation", SPACING_SMALL)

# Apply standard configuration for entity buttons
func setup_entity_button(button: Button, is_active: bool = false) -> void:
	button.theme_type_variation = "_entity_button"
	button.custom_minimum_size = ENTITY_BUTTON_MIN_SIZE
	button.size_flags_horizontal = Control.SIZE_SHRINK_CENTER
	button.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	
	if is_active:
		# Get from theme or create style for active state
		var theme = button.get_theme()
		if theme:
			var active_style = theme.get_stylebox("active", "_entity_button")
			if active_style:
				button.add_theme_stylebox_override("normal", active_style)
	
# Truncate entity name for display
func format_entity_name(entity_name: String) -> String:
	if entity_name.length() > ENTITY_BUTTON_TEXT_MAX_LENGTH:
		return entity_name.substr(0, ENTITY_BUTTON_TEXT_MAX_LENGTH - 2) + ".."
	return entity_name 
