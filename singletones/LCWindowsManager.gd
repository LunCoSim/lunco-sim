class_name LCWindowsManager
extends Node


static func make_window(control, title):
	var win: PankuLynxWindow = Panku.windows_manager.create_window(control)
	
	var size = control.get_minimum_size() + Vector2(0, win._window_title_container.get_minimum_size().y)
	win.set_custom_minimum_size(size)
	win.size = win.get_minimum_size()

	win.set_window_title_text(title)
	win.show_window()
