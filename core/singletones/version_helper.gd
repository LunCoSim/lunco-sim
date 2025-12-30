class_name LCVersionHelper
extends Object

const GIT_HEAD_FILE_PATH = "res://.git/HEAD"
const GIT_DIR_PATH = "res://.git/"

static var _git_hash: String = ""
static var _initialized: bool = false

static func get_git_hash() -> String:
	_ensure_init()
	return _git_hash

static func get_version_string() -> String:
	_ensure_init()
	var ver = "v" + str(ProjectSettings.get_setting("application/config/version"))
	if _git_hash:
		ver += " (%s)" % _git_hash
	return ver

static func _ensure_init():
	if _initialized:
		return
	_git_hash = _fetch_git_hash()
	if _git_hash != "":
		print("[VersionHelper] Current Git Hash: ", _git_hash)
	_initialized = true

static func _fetch_git_hash() -> String:
	# 1. Try OS.execute (works in editor/dev builds where git is available)
	# Skip on Web platform as OS.execute is not available
	if not OS.has_feature("web"):
		var output = []
		var exit_code = OS.execute("git", ["rev-parse", "--short", "HEAD"], output)
		
		if exit_code == 0 and output.size() > 0:
			var hash_str = output[0].strip_edges()
			if hash_str.length() > 0:
				return hash_str
			
	# 2. Fallback: Try reading res://git_hash.txt (generated during build)
	if FileAccess.file_exists("res://git_hash.txt"):
		var file = FileAccess.open("res://git_hash.txt", FileAccess.READ)
		if file:
			var hash_str = file.get_as_text().strip_edges()
			if hash_str.length() > 0:
				return hash_str
	
	return ""
