extends Control

@onready var message_label = $Panel/Message
@onready var panel = $Panel

var display_time = 3.0  # Seconds to display the notification
var fade_time = 0.5  # Seconds to fade out
var timer = 0.0

var tween: Tween = null

func _ready():
	# Initially hide the notification
	panel.modulate.a = 0
	
func _process(delta):
	if timer > 0:
		timer -= delta
		if timer <= fade_time and tween == null:
			# Start fade out
			tween = create_tween()
			tween.tween_property(panel, "modulate:a", 0, fade_time)
			tween.finished.connect(_on_tween_finished)
			
func show_message(text: String, duration: float = 3.0):
	# Stop any existing tween
	if tween:
		tween.kill()
		tween = null
	
	# Set the message
	message_label.text = text
	
	# Set the display time
	display_time = duration
	timer = display_time
	
	# Show the notification with a fade in
	panel.modulate.a = 0
	tween = create_tween()
	tween.tween_property(panel, "modulate:a", 1, 0.2)
	tween.finished.connect(_on_fade_in_finished)

func _on_fade_in_finished():
	tween = null

func _on_tween_finished():
	tween = null 