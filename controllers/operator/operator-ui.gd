extends Control

signal model_selected(path)

var target
var leaves = {}
var ModelsList = []

func set_target(_target):
	target = _target

#SRC: https://godotengine.org/qa/5175/how-to-get-all-the-files-inside-a-folder
#Listing all files
func list_files_in_directory(path, files=[]):
#	var dir = Directory.new()
#	dir.open(path)
#	dir.list_dir_begin(true, true) #filtering files
#
#	while true:
#		var file = dir.get_next()
#		if file == "":
#			break
#		else:
#			if dir.dir_exists(file):
#				list_files_in_directory(path+file+"/", files)
#			files.append({
#				"path": path,
#				"filename": file,
#				"extension": file.split(".")[-1]
#			})
#
#	dir.list_dir_end()
#
#	return files
	return []

func filter_by_extension(files: Array, extension="escn"):
	var res = []
	for file in files:
		if(file["extension"]==extension):
			res.append(file)
	return res

func get_leave(tree, path):
	if(path in leaves):
		return leaves[path]
	else:
		var dirs = path.replace("res://").split("/")
		dirs[0] = "res://" + dirs[0]
		for d in dirs:
			if(path in leaves):
				return leaves[path]

# Called when the node enters the scene tree for the first time.
func _ready():
	var path = "res://addons/"
	var files = list_files_in_directory(path)

	ModelsList = filter_by_extension(files)

	var tree = $PanelContainer2/Files

	var root = tree.create_item()
	root.set_text(0, "Models")

	for file in ModelsList:
		var child = tree.create_item()
		child.set_text(0, file["filename"])
		leaves[file["filename"]] = child




# Called every frame. 'delta' is the elapsed time since the previous frame.
#func _process(delta):
#	pass


func _on_Files_button_pressed(item, column, id):
	print(column, id)


func _on_Files_cell_selected():
	var item = $Files.get_selected()
	var text = item.get_text(0)

	for m in ModelsList:
		if(m["filename"] == text):
			emit_signal("model_selected", m["path"] + m["filename"])


func _on_Files_item_activated():
	pass # Replace with function body.
