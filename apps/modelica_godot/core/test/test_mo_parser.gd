@tool
extends SceneTree

const MOParser = preload("res://core/mo_parser.gd")
var parser: MOParser

func _init() -> void:
	print("Starting test")
	parser = MOParser.new()
	get_root().add_child(parser)
	test_simple()
	quit()

func test_simple() -> void:
	print("Testing simple model")
	var model_text: String = """
	model SimpleModel "Test"
		Real x;
	equation
		der(x) = -x;
	end SimpleModel;
	"""
	var result = parser.parse_text(model_text)
	assert_eq(result.type, "model", "Model type")

func assert_eq(actual, expected, message: String) -> void:
	if actual != expected:
		push_error("Assertion failed: " + message) 