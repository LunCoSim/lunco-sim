extends PanelContainer

func _ready():
	# Load hide tutorial setting from Profile
	%HideTutorial.button_pressed = Profile.hide_tutorial

func _on_hide_tutorial_toggled(toggled_on):
	# Save the setting to Profile
	Profile.hide_tutorial = toggled_on


func _on_rich_text_label_meta_clicked(meta):
	OS.shell_open(str(meta))
