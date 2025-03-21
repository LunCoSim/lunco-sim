extends Node

var notification_scene = preload("res://core/widgets/notification.tscn")
var current_notification = null

func _ready():
	pass

func show_notification(message: String, duration: float = 3.0):
	# Print to console for logging
	print("[Notification] " + message)
	
	# Create visual notification
	if not current_notification:
		current_notification = notification_scene.instantiate()
		get_tree().root.add_child(current_notification)
	
	current_notification.show_message(message, duration) 