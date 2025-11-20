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
const CORNER_RADIUS_SMALL = 6
const CORNER_RADIUS_MEDIUM = 10
const CORNER_RADIUS_LARGE = 16

# Color variants - centralize color management
const COLOR_BG = Color(0.12, 0.15, 0.2, 0.9)
const COLOR_BG_LIGHT = Color(0.15, 0.18, 0.23, 0.9)
const COLOR_BORDER = Color(0.25, 0.28, 0.32, 1.0)
const COLOR_ACCENT = Color(0.2, 0.4, 0.8, 0.9)
const COLOR_TEXT = Color(0.9, 0.9, 0.95, 1.0)

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
	var style = StyleBoxFlat.new()
	style.content_margin_left = MARGIN_MEDIUM
	style.content_margin_top = MARGIN_MEDIUM
	style.content_margin_right = MARGIN_MEDIUM
	style.content_margin_bottom = MARGIN_MEDIUM
	style.bg_color = COLOR_BG
	style.border_width_left = 1
	style.border_width_top = 1
	style.border_width_right = 1
	style.border_width_bottom = 1
	style.border_color = COLOR_BORDER
	style.corner_radius_top_left = CORNER_RADIUS_MEDIUM
	style.corner_radius_top_right = CORNER_RADIUS_MEDIUM
	style.corner_radius_bottom_right = CORNER_RADIUS_MEDIUM
	style.corner_radius_bottom_left = CORNER_RADIUS_MEDIUM
	style.shadow_color = Color(0, 0, 0, 0.2)
	style.shadow_size = 6
	panel.add_theme_stylebox_override("panel", style)

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