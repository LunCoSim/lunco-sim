# LunCo Logger (LCLogger)

A modern, asynchronous logging system for Godot 4.x. It leverages the latest engine features like `OS.add_logger()` to capture all engine output while providing a flexible, endpoint-based architecture for custom log handling.

## Features

- **🎯 Engine Integration:** Uses `OS.add_logger()` and `Logger` class to intercept all `print()`, `push_error()`, and internal engine messages.
- **🔍 Source Tracking:** Automatically captures the source of every log event, including filename, function name, and line number (e.g., `[main_menu.gd:_ready:42]`).
- **⚡ Asynchronous:** Logs are processed in a dedicated worker thread using `WorkerThreadPool`, ensuring logging never blocks your main game loop.
- **📁 File Logging:** Built-in support for rotating log files with configurable paths and maximum file counts.
- **🛠 Customizable Endpoints:** Easily extend the logger by adding custom endpoints (e.g., Discord webhooks, remote telemetry, or custom in-game consoles).
- **📊 Log Levels:** Granular control over log levels (`DEBUG`, `INFO`, `WARN`, `ERROR`, `FATAL`) for each endpoint.
- **📝 Thread-Safe:** Built with `Mutex` and `Semaphore` to handle logging from any thread safely.

## Installation

### Via [gd-plug](https://github.com/imjp94/gd-plug) (Recommended)

To install `LCLogger` separately from the main LunCo repository, add the following to your `plug.gd`:

```gdscript
func _plugging():
    plug("LunCo/lunco-sim", {"include": ["addons/lunco-logger"]})
```

### Manual Installation

1. Copy the `addons/lunco-logger` directory into your project's `addons/` folder.
2. Enable the plugin in **Project Settings > Plugins**.
3. The `LCLogger` singleton will be automatically registered.

## Configuration

You can configure the logger directly in **Project Settings** under the `lunco/logger/` section:

- `lunco/logger/path`: Directory where log files are stored (default: `user://logs`).
- `lunco/logger/max_files`: Number of rotated log files to keep.
- `lunco/logger/stdout_level`: Log level filter for the standard output (console).
- `lunco/logger/file_level`: Log level filter for the log file.

## Usage

### Simple Logging

The `LCLogger` singleton provides a clean API for different log levels:

```gdscript
LCLogger.debug("This is a debug message")
LCLogger.info("Application started")
LCLogger.warn("Low memory detected")
LCLogger.error("Failed to load resource: ", resource_path)
LCLogger.fatal("Unrecoverable error occurred!")
```

### Variadic Arguments & JSON Support

`LCLogger` automatically stringifies dictionaries and arrays and joins multiple arguments:

```gdscript
# In your script (e.g., player.gd at line 42)
var data = {"id": 42, "status": "ok"}
LCLogger.info("Received data: ", data) 
# Output: [12:34:56.789][INFO][player.gd:_ready:42] Received data: {"id": 42, "status": "ok"}
```

### Full Engine Redirection

All standard `print()` calls are automatically captured and enhanced with timestamps and level indicators:

```gdscript
print("Normal engine print")
# Output: [12:34:56.789][INFO][player.gd:_ready:45] Normal engine print
```

### Adding Custom Endpoints

You can create your own endpoint by extending `LCLogEndpoint`:

```gdscript
class MyCustomEndpoint extends LCLogEndpoint:
    func _log_message(message: String, is_error: bool):
        # Your custom logic here (e.g., send to a server)
        pass

func _ready():
    var my_endpoint = MyCustomEndpoint.new()
    my_endpoint.level_mask = LCLogger.LogLevel.ERROR | LCLogger.LogLevel.FATAL
    LCLogger.add_endpoint(my_endpoint)
```

## License

This addon is part of the LunCo project and is licensed under the MIT License.
