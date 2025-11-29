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

# Corner radii - glassmorphic style (larger, smoother)
const CORNER_RADIUS_SMALL = 8
const CORNER_RADIUS_MEDIUM = 12
const CORNER_RADIUS_LARGE = 16

# Color variants - centralize color management (Glassmorphic Theme)
const COLOR_BG = Color(0.06, 0.08, 0.12, 0.96)  # More opaque dark blue
const COLOR_BG_LIGHT = Color(0.12, 0.15, 0.22, 0.95)  # More opaque lighter blue
const COLOR_BORDER = Color(0.25, 0.6, 0.95, 0.6)  # Neon cyan border
const COLOR_BORDER_GLOW = Color(0.3, 0.7, 1.0, 0.8)  # Bright cyan glow
const COLOR_ACCENT = Color(0.25, 0.45, 0.75, 0.95)  # Brighter blue accent (matches active buttons)
const COLOR_ACCENT_BRIGHT = Color(0.4, 0.8, 1.0, 1.0)  # Bright neon accent
const COLOR_TEXT = Color(0.95, 0.95, 0.98, 1.0)  # Brighter text for contrast
const COLOR_SHADOW = Color(0.15, 0.5, 0.9, 0.4)  # Blue glow shadow

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
	button.custom_minimum_size = Vector2(0, 40) # Allow width to be dynamic
	button.size_flags_horizontal = Control.SIZE_SHRINK_BEGIN # Minimal width to fit text
	button.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	
	if is_active:
		# Highlight active button
		button.modulate = COLOR_ACCENT
	else:
		button.modulate = Color(1, 1, 1, 1)
	
# Truncate entity name for display
func format_entity_name(entity_name: String) -> String:
	return entity_name 
