@tool
extends EditorPlugin

const AUTOLOAD_NAME = "LCLogger"

func _enter_tree() -> void:
	add_autoload_singleton(AUTOLOAD_NAME, "res://addons/lunco-logger/lc_log_manager.gd")

func _exit_tree() -> void:
	remove_autoload_singleton(AUTOLOAD_NAME)
