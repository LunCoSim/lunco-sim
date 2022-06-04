extends Node

#SRC: https://godotengine.org/qa/5175/how-to-get-all-the-files-inside-a-folder
#Listing all files
func list_files_in_directory(path, files=[]):
	var dir = Directory.new()
	dir.open(path)
	dir.list_dir_begin(true, true) #filtering files

	while true:
		var file = dir.get_next()
		if file == "":
			break
		else:
			if dir.dir_exists(file):
				list_files_in_directory(path+file+"/", files)
			files.append({
				"path": path, 
				"filename": file,
				"extension": file.split(".")[-1]
			})
			
	dir.list_dir_end()

	return files

func filter_by_extension(files: Array, extension="escn"):
	var res = []
	for file in files:
		if(file["extension"]==extension):
			res.append(file)
	return res
	
# Called when the node enters the scene tree for the first time.
func _ready():
	#pass # Replace with function body.
	var path = "res://addons/"
	var files = list_files_in_directory(path)
	
	var models = filter_by_extension(files)
	
	var tree = $Control/Files
	
	for file in models:
		var subchild1 = tree.create_item()
		subchild1.set_text(0, file["filename"])
		
	
		
# Called every frame. 'delta' is the elapsed time since the previous frame.
#func _process(delta):
#	pass
