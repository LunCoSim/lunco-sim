extends SceneTree

var package_manager: PackageManager

func _init():
	print("\n=== Starting Package Loading Test ===")
	
	# Create package manager
	package_manager = PackageManager.new()
	get_root().add_child(package_manager)
	
	# Load the root package
	var package_path = "res://apps/modelica_godot/components"
	print("\nLoading package from: ", package_path)
	var success = package_manager.load_package(package_path)
	
	if success:
		print("\nPackage loaded successfully!")
		print("\nLoaded packages:")
		for package in package_manager.get_loaded_packages():
			print("- ", package)
			var metadata = package_manager.get_package_metadata(package)
			print("  Within: ", metadata.get("within", ""))
			print("  Path: ", metadata.get("path", ""))
	else:
		print("\nFailed to load package!")
	
	print("\n=== Test Complete ===")
	quit() 