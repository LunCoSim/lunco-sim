#!/usr/bin/env -S godot --headless --script
extends SceneTree

func _init():
	var base_test_path = ProjectSettings.globalize_path("res://apps/modelica/tests/base_test.gd")
	print("Loading base test from: " + base_test_path)
	
	var BaseTest = load("res://apps/modelica/tests/base_test.gd")
	if BaseTest:
		print("Successfully loaded BaseTest class")
		BaseTest.run_all_tests()
	else:
		print("Failed to load BaseTest class")
		
	quit()