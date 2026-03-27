extends SceneTree

func _init():
	var logger_script = load("res://addons/lunco-logger/lc_log_manager.gd")
	var logger = logger_script.new()
	logger.name = "TestLogger"
	root.add_child.call_deferred(logger)
	
	# Wait for initialization
	var t = Time.get_ticks_msec()
	while Time.get_ticks_msec() - t < 500:
		pass
		
	print("--- TEST LOGGER START ---")
	logger.info("This is logger.info")
	logger.warn("This is logger.warn")
	logger.error("This is logger.error")
	
	print("Direct print for comparison")
	
	# Wait for processing
	t = Time.get_ticks_msec()
	while Time.get_ticks_msec() - t < 1000:
		pass
		
	logger.flush()
	print("--- TEST LOGGER END ---")
	quit()
