extends Control

signal nft_issued(nft_data)

@onready var name_input: LineEdit = $Panel/VBoxContainer/NameInput
@onready var color_picker: ColorPickerButton = $Panel/VBoxContainer/ColorPicker
@onready var issue_button: Button = $Panel/VBoxContainer/IssueButton

func _ready():
	issue_button.connect("pressed", Callable(self, "_on_issue_button_pressed"))

func _on_issue_button_pressed():
	var nft_data = {
		"name": name_input.text,
		"color": color_picker.color.to_html(false)
	}
	emit_signal("nft_issued", nft_data)
	queue_free()  # Close the popup after issuing

func _input(event):
	if event is InputEventMouseButton and event.pressed and event.button_index == MOUSE_BUTTON_LEFT:
		# Check if the click is outside the popup
		if not get_global_rect().has_point(event.position):
			queue_free()  # Close the popup if clicked outside
