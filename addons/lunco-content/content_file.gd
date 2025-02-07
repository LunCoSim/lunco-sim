class_name ContentFile
extends RefCounted

var url: String
var size: int
var checksum: String
var metadata: Dictionary

func load(path: String) -> Error:
    if not FileAccess.file_exists(path):
        return ERR_FILE_NOT_FOUND
        
    var config = ConfigFile.new()
    var err = config.load(path)
    if err != OK:
        return err
    
    # Load source information
    url = config.get_value("source", "url", "")
    size = config.get_value("source", "size", 0)
    checksum = config.get_value("source", "checksum", "")
    
    # Load metadata if present
    if config.has_section("metadata"):
        for key in config.get_section_keys("metadata"):
            metadata[key] = config.get_value("metadata", key)
    
    return OK

func save(path: String) -> Error:
    var config = ConfigFile.new()
    
    # Save source information
    config.set_value("source", "url", url)
    config.set_value("source", "size", size)
    config.set_value("source", "checksum", checksum)
    
    # Save metadata
    for key in metadata:
        config.set_value("metadata", key, metadata[key])
    
    return config.save(path) 