# LunCo: Used terms

1. **Matrix** – an instance of a simulated world in Godot. Can be on a remote server or local
2. **Terminal** – Application used by a user to connect to a Matrix
3. **Avatar** – user's representetive in Matrix allowing interaction with the world
4. **System** – system that is modeled, like rockets, robots, or just enviroment
5. **State** – Commands changes system state and actions
6. **Commands** – some systems can execute commmands
   1. command arguments
7. **Actions** – system can do actions. It's different actions than Godot
8. **Parameters** – systems are represented using parameters
   1.  *telemetered* – coming from remote system
   2.  *derived* – computed by algorithms
   3.  *constant* – defined in mission database
   4.  *system* – parameters of Lunco
9. **Telemetry** – Sampled values of System obtained from 
   1. *Stream* – flow of telemetered parameters
10. **Digital twin** or just **twin** – detailed representation of a system, often several aspects combined e.g. cad+thermal model
11. **Model** – a part of a digital twin, specific representation of a system, e.g. cad model, thermal model. 
12. **Viewport**
    1.  *Camera*
    2.  *UI*
    3.  *Widget*
13. **Mission Database (MDB)** – The Mission Database contains the description of the telecommands and telemetry including calibration curves, algorithms, limits, alarms, constraints, command pre and post verification.

LunCo simulates the world and it's called Matrix. User connects to Matrix server using Therminal. 
User is represented by an avatar that can interact with digital twins of system living in Matrix through Commands to ask them to perform Actions and monitor the result as telemetry.

Terminal renders what Avatar sees in the Viewport from Cameras. On top of Viewport there is a UI layer that visualises data using Widgets.
## Controled by **Avatar**

1. Player – a person like character in game.
2. Vehicle
   1. Rocket
   2. Rover
3. Building
4. Operator – god like character, can do everything


## View
1. Third-person view
2. Strategic view


## References

1. [Monitoring and Control Model](https://docs.yamcs.org/yamcs-server-manual/general/model/)
2. [XML TELEMETRIC AND COMMAND EXCHANGE™ (XTCE™)](https://www.omg.org/xtce/index.htm)
3. [Consultative Committee for Space Data Systems (CCSDS)](https://en.wikipedia.org/wiki/Consultative_Committee_for_Space_Data_Systems)