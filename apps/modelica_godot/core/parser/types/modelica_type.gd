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

static var _builtin_types: Dictionary = {}

static func get_builtin_type(name: String) -> ModelicaTypeClass:
	if _builtin_types.is_empty():
		_init_builtin_types()
	return _builtin_types.get(name)

static func _init_builtin_types() -> void:
	# Create basic types
	var real = ModelicaTypeClass.new()
	real.kind = TypeKind.BUILTIN
	real.name = "Real"
	real.has_default = true
	real.default_value = 0.0
	_builtin_types["Real"] = real
	
	var integer = ModelicaTypeClass.new()
	integer.kind = TypeKind.BUILTIN
	integer.name = "Integer"
	integer.is_discrete = true
	integer.has_default = true
	integer.default_value = 0
	_builtin_types["Integer"] = integer
	
	var boolean = ModelicaTypeClass.new()
	boolean.kind = TypeKind.BUILTIN
	boolean.name = "Boolean"
	boolean.is_discrete = true
	boolean.has_default = true
	boolean.default_value = false
	_builtin_types["Boolean"] = boolean
	
	var string = ModelicaTypeClass.new()
	string.kind = TypeKind.BUILTIN
	string.name = "String"
	string.has_default = true
	string.default_value = ""
	_builtin_types["String"] = string

static func create_array_type(element_type: ModelicaTypeClass, dimensions: Array) -> ModelicaTypeClass:
	var array_type = ModelicaTypeClass.new()
	array_type.kind = TypeKind.ARRAY
	array_type.name = element_type.name + "[]"
	array_type.element_type = element_type
	array_type.dimensions = dimensions
	return array_type

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