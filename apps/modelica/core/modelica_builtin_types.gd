@tool
extends RefCounted
class_name ModelicaBuiltinTypes

const ModelicaTypeClass = preload("res://apps/modelica/core/modelica_type.gd")

# Dictionary to store all built-in types
static var _types: Dictionary = {}

# Initialize the built-in types
static func init() -> void:
	if not _types.is_empty():
		return  # Already initialized
	
	# Create Real type
	var real = ModelicaTypeClass.new()
	real.kind = ModelicaTypeClass.TypeKind.BUILTIN
	real.name = "Real"
	real.has_default = true
	real.default_value = 0.0
	_types["Real"] = real
	
	# Create Integer type
	var integer = ModelicaTypeClass.new()
	integer.kind = ModelicaTypeClass.TypeKind.BUILTIN
	integer.name = "Integer"
	integer.is_discrete = true
	integer.has_default = true
	integer.default_value = 0
	_types["Integer"] = integer
	
	# Create Boolean type
	var boolean = ModelicaTypeClass.new()
	boolean.kind = ModelicaTypeClass.TypeKind.BUILTIN
	boolean.name = "Boolean"
	boolean.is_discrete = true
	boolean.has_default = true
	boolean.default_value = false
	_types["Boolean"] = boolean
	
	# Create String type
	var string = ModelicaTypeClass.new()
	string.kind = ModelicaTypeClass.TypeKind.BUILTIN
	string.name = "String"
	string.has_default = true
	string.default_value = ""
	_types["String"] = string

# Get a built-in type by name
static func get_type(name: String) -> ModelicaTypeClass:
	if _types.is_empty():
		init()
	return _types.get(name)

# Check if a type name is a built-in type
static func is_builtin_type(name: String) -> bool:
	if _types.is_empty():
		init()
	return _types.has(name)

# Create an array type
static func create_array_type(element_type: ModelicaTypeClass, dimensions: Array) -> ModelicaTypeClass:
	var array_type = ModelicaTypeClass.new()
	array_type.kind = ModelicaTypeClass.TypeKind.ARRAY
	array_type.name = element_type.name + "[]"
	array_type.element_type = element_type
	array_type.dimensions = dimensions
	return array_type