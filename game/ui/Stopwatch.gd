extends Label

var time_elapsed := 0.0

func _process(delta: float) -> void:
	time_elapsed += delta
	var minutes := time_elapsed / 60
	var seconds := fmod(time_elapsed, 60)
	# $Label.set(text, "%02d:%02d" % [minutes, seconds])
	# $Label.Text = "%02d:%02d" % [minutes, seconds]
	text = "MET: %02d:%02d" % [minutes, seconds]
