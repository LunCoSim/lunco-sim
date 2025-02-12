@tool
extends Node
class_name WorkspaceConfig

# Standard Modelica workspace paths
const MODELICA_PATHS = {
	"MSL": "MSL",                    # Modelica Standard Library
	"USER_LIBS": "Libraries",        # User-created libraries
	"MODELS": "Models",              # User models
	"RESOURCES": "Resources",        # Non-Modelica resources
	"CACHE": "Cache",                # Cache directory
	"RESULTS": "Results"             # Simulation results
}

# Required workspace structure
const REQUIRED_DIRS = [
	"MSL",
	"Libraries",
	"Models",
	"Resources",
	"Cache",
	"Results"
]

var workspace_root: String
var is_initialized: bool = false

func initialize(root_path: String) -> bool:
	workspace_root = root_path
	return _validate_and_create_structure()

func get_path(path_type: String) -> String:
	if not MODELICA_PATHS.has(path_type):
		push_error("Invalid path type: " + path_type)
		return ""
	return workspace_root.path_join(MODELICA_PATHS[path_type])

func _validate_and_create_structure() -> bool:
	# Check if workspace root exists
	var dir = DirAccess.open(workspace_root)
	if not dir:
		push_error("Cannot access workspace root: " + workspace_root)
		return false
	
	# Create required directories
	for required_dir in REQUIRED_DIRS:
		var path = workspace_root.path_join(required_dir)
		if not DirAccess.dir_exists_absolute(path):
			var err = DirAccess.make_dir_recursive_absolute(path)
			if err != OK:
				push_error("Failed to create directory: " + path)
				return false
	
	# Create package.mo in workspace root if it doesn't exist
	var root_package = workspace_root.path_join("package.mo")
	if not FileAccess.file_exists(root_package):
		var file = FileAccess.open(root_package, FileAccess.WRITE)
		if file:
			file.store_string(_generate_root_package())
		else:
			push_error("Failed to create root package.mo")
			return false
	
	is_initialized = true
	return true

func _generate_root_package() -> String:
	var workspace_name = workspace_root.get_file()
	return """package %s "Root package for %s workspace"
  annotation(
    Documentation(info="Modelica workspace for %s project.
    Contains:
    - Models: User-created models
    - Libraries: User-created libraries
    - MSL: Modelica Standard Library
    - Resources: Additional resources
    ")
  );
end %s;
""" % [workspace_name, workspace_name, workspace_name, workspace_name] 