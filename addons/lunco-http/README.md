# LunCo HTTP Server (LCHttp)

A lightweight, routable HTTP/1.1 server for Godot 4.x, supporting both HTTP and HTTPS (TLS). Designed for building internal tools, telemetry systems, and signaling servers directly within the Godot Engine.

## Features

- **🚀 Easy Routing:** Express-like routing system using `LCHttpRouter`.
- **🔒 HTTPS Support:** Built-in TLS support via `StreamPeerTLS`.
- **📂 File Serving:** Dedicated `LCHttpFileRouter` for serving static assets from `res://` or `user://`.
- **📨 Request/Response Abstraction:** Clean `LCHttpRequest` and `LCHttpResponse` classes for handling headers, query parameters, and JSON bodies.
- **⚡ Non-blocking:** Runs within the Godot SceneTree, utilizing the main loop for asynchronous I/O.
- **🛠 Modular:** Part of the LunCo ecosystem but fully functional as a standalone addon.

## Installation

### Via [gd-plug](https://github.com/imjp94/gd-plug) (Recommended)

To install `LCHttp` separately from the main LunCo repository, add the following to your `plug.gd`:

```gdscript
func _plugging():
    plug("LunCo/lunco-sim", {"include": ["addons/lunco-http"]})
```

### Manual Installation

1. Copy the `addons/lunco-http` directory into your project's `addons/` folder.
2. Enable the plugin in **Project Settings > Plugins**.

## Quick Start

### Basic HTTP Server

```gdscript
extends Node

func _ready():
    var server = LCHttpServer.new()
    
    # Register a simple router
    server.register_router("/hello", MyHelloRouter.new())
    
    # Serve static files from a directory
    server.register_router("/web", LCHttpFileRouter.new("res://www/"))
    
    add_child(server)
    server.start(8080)

class MyHelloRouter extends LCHttpRouter:
    func handle_get(request: LCHttpRequest, response: LCHttpResponse):
        response.send("<h1>Hello from Godot!</h1>")
```

### Configuring HTTPS

To use HTTPS, provide paths to your SSL certificate and private key:

```gdscript
func _ready():
    var server = LCHttpServer.new()
    var err = server.configure_tls("res://certs/server.crt", "res://certs/server.key")
    
    if err == OK:
        add_child(server)
        server.start(443)
```

## Naming Convention

All classes are prefixed with `LC` (e.g., `LCHttpServer`, `LCHttpRequest`) to avoid naming collisions with Godot's built-in `HTTPRequest` client or other community addons like `gdUnit4`.

## License

This addon is part of the LunCo project and is licensed under the MIT License.
