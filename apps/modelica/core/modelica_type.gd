@tool
extends RefCounted
class_name ModelicaTypeClass

enum TypeKind {
	UNKNOWN,
	ERROR,
	BUILTIN,      # Built-in types like Real, Integer, etc.
	ARRAY,        # Array types
	RECORD,       # Record types
	CLASS,        # Class types
	CONNECTOR,    # Connector types
	MODEL,        # Model types
	BLOCK,        # Block types
	TYPE,         # Type definitions
	FUNCTION      # Function types
}

# Basic type properties
var kind: TypeKind = TypeKind.UNKNOWN
var name: String = ""
var qualified_name: String = ""

# Type hierarchy
var base_type = null  # Base type for inheritance (ModelicaTypeClass)
var type_parameters: Array = []  # For generic/parameterized types

# For array types
var element_type = null  # Type of array elements (ModelicaTypeClass)
var dimensions: Array = []  # Array dimensions

# For record/class/model types
var fields: Dictionary = {}  # Field name -> ModelicaTypeClass
var methods: Dictionary = {}  # Method name -> ModelicaTypeClass

# Type constraints and modifiers
var constraints: Array = []  # Type constraints
var modifiers: Dictionary = {}  # Type modifiers

# Built-in type properties
var is_discrete: bool = false
var has_default: bool = false
var default_value = null

# Load built-in types
const BuiltinTypes = preload("res://apps/modelica/core/modelica_builtin_types.gd")

# Static method to get built-in type - forwards to ModelicaBuiltinTypes
static func get_builtin_type(name: String) -> ModelicaTypeClass:
	return BuiltinTypes.get_type(name)

# Static method to create array type - forwards to ModelicaBuiltinTypes
static func create_array_type(element_type: ModelicaTypeClass, dimensions: Array) -> ModelicaTypeClass:
	return BuiltinTypes.create_array_type(element_type, dimensions)

func is_numeric() -> bool:
	return name in ["Real", "Integer"]

func is_compatible_with(other: ModelicaTypeClass) -> bool:
	# Same type
	if self == other:
		return true
	
	# Handle array types
	if kind == TypeKind.ARRAY and other.kind == TypeKind.ARRAY:
		return element_type.is_compatible_with(other.element_type)
	
	# Handle inheritance
	var current = self
	while current:
		if current == other:
			return true
		current = current.base_type
	
	return false

func _to_string() -> String:
	match kind:
		TypeKind.ARRAY:
			var dim_str = ""
			for d in dimensions:
				dim_str += "[" + str(d) + "]"
			return str(element_type) + dim_str
		_:
			return name if not qualified_name else qualified_name 