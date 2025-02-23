class_name ModelicaBase
extends RefCounted

enum AccessLevel {
    PUBLIC,
    PROTECTED
}

class Declaration:
    var name: String
    var access: AccessLevel
    var binding_equation: String  # Optional equation that binds the value
    var description: String
    
    func _init(p_name: String, p_access: AccessLevel = AccessLevel.PUBLIC) -> void:
        name = p_name
        access = p_access
        binding_equation = ""
        description = ""

var declarations: Dictionary = {}  # name -> Declaration

func add_declaration(decl: Declaration) -> void:
    declarations[decl.name] = decl

func get_declaration(name: String) -> Declaration:
    return declarations.get(name)

func has_declaration(name: String) -> bool:
    return declarations.has(name)

func _to_string() -> String:
    var result = "ModelicaBase:\n"
    for decl in declarations.values():
        result += "  %s (%s)\n" % [decl.name, "public" if decl.access == AccessLevel.PUBLIC else "protected"]
    return result 