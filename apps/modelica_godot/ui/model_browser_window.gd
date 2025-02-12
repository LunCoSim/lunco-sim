@tool
class_name ModelBrowserWindow
extends Window

signal model_selected(model_path: String, model_data: Dictionary)

@onready var model_browser = $ModelBrowser

func _ready():
	# Connect window close button
	close_requested.connect(_on_close_requested)
	
	# Forward model selected signal
	model_browser.model_selected.connect(_on_model_selected)

func initialize(model_manager: ModelManager) -> void:
	model_browser.initialize(model_manager)

func _on_close_requested():
	hide()

func _on_model_selected(model_path: String, model_data: Dictionary):
	emit_signal("model_selected", model_path, model_data) 
