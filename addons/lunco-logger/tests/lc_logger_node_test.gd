extends Node

func _ready():
	print("--- TEST LOGGER START ---")
	LCLogger.info("This is logger.info")
	LCLogger.warn("This is logger.warn")
	LCLogger.error("This is logger.error")
	
	print("Direct print for comparison")
	
	# Wait for worker thread
	await get_tree().create_timer(1.0).timeout
	
	LCLogger.flush()
	print("--- TEST LOGGER END ---")
	get_tree().quit()
