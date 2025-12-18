# Custom Models & Entities

LunCoSim is designed to be extensible. You can add new vehicles, buildings, or characters by registering them with the **Entities Manager**.

## The Entities Manager
Located at `core/singletones/entities-manager.gd`, this singleton maintains the registry of all spawnable objects.

### Adding a New Entity

1.  **Define the Entity**: Add a new entry to the `Entities` enum.
    ```gdscript
    enum Entities {
        Spacecraft,
        ...
        MyNewRover, // <--- Add this
    }
    ```

2.  **Register Paths**: Update the `Paths`, `UIs`, and `InputAdapters` dictionaries.
    ```gdscript
    var Paths = {
        ...
        Entities.MyNewRover: "res://mods/my_new_rover/rover.tscn",
    }
    ```

## Entity Requirements
A valid entity scene generally requires:
-   **Root Node**: Typically a `RigidBody3D` or `VehicleBody3D`.
-   **Controller**: A script extending `LCController` to handle logic.
-   **Input Adapter**: A script extending `LCInputAdapter` to map user inputs to controller actions.
-   **Effectors**: (Optional) `LCEffector` child nodes for simulation integration (Power, Thermal).

## Custom Controllers
Create a new script extending `LCController` (or `LCRoverController`, etc.).
```gdscript
extends LCRoverController

func _physics_process(delta):
    # Your custom movement logic here
    pass
```
