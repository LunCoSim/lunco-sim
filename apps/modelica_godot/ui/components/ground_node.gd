class_name GroundGraphNode
extends ComponentGraphNode

func _init(component_: GroundComponent):
	super(component_)
	
	# Add ground symbol
	var symbol = Label.new()
	symbol.text = "⏚"  # Ground symbol
	symbol.add_theme_font_size_override("font_size", 24)
	add_child(symbol) 
