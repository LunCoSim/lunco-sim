extends Label

var time_elapsed := 0.0

func _process(delta: float) -> void:
	time_elapsed += delta
	var seconds := fmod(time_elapsed, 60)
	var minutes := time_elapsed / 60
	var hours := minutes / 60
	text = "%02d:%02d:%02d" % [hours, minutes, seconds]
