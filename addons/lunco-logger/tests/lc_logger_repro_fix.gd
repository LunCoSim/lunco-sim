extends SceneTree

func _init():
	# Manually load and instantiate the logger manager
	var logger_script = load("res://addons/lunco-logger/lc_log_manager.gd")
	var logger = logger_script.new()
	logger.name = "TestLogger"
	
	# Add to scene tree
	root.add_child.call_deferred(logger)
	
	print("--- TESTING START ---")
	print("This is a direct print - should appear ONCE")
	
	# Give it a moment to initialize (it uses _enter_tree)
	var t = Time.get_ticks_msec()
	while Time.get_ticks_msec() - t < 200:
		pass
	
	# Variadic calls
	logger.info("This is logger.info - should appear ONCE and BE FORMATTED")
	
	logger.debug("This is logger.debug with multiple", " args ", {"key": "value"})
	
	printerr("This is a direct printerr - should appear ONCE as Godot error")
	
	logger.error("This is logger.error - should appear ONCE as our formatted error")
	
	# Give it a moment to process in the worker thread
	t = Time.get_ticks_msec()
	while Time.get_ticks_msec() - t < 1000:
		pass
	
	logger.flush()
	
	print("--- TESTING END ---")
	
	# Cleanup
	logger.queue_free()
	
	quit()
