# LunCo: full-cycle space operations sim tool

An open-source Lunar Colony sim built for **engineers** and **enthusiasts**

*Inspired by*
* Games: KSP, SimCity, Factorio
* Professional: AGI, Solidworks, Ansys

Free for **commercial use**.

***[OUR GITCOINT GRANT](https://gitcoin.co/grants/5939/lunco-full-cycle-space-operations-sim-tool)***

*PS Making Kerbal a real thing. Intended for industry applications*

*[DOWNLOAD BINARY](https://github.com/lun-co/lunco-game/releases)*

*click on the image below to watch first gameplay video on youtube*

[![First gameplay video](docs/assets/second_screenshot.jpg "First gameplay video")](https://www.youtube.com/watch?v=PmTgnI-z-rY&feature=youtu.be "First gameplay video! - Click to Watch!")

## Features
* Two game play stules
  * Digital twin mode for engineers
  * Story mode for gamers
* Realistic space exploration engine
* User generated content: space crafts, trajectories, factories, etc.
* IPFS for CDN
* Radicle to track history of user generated content
* Precise model of a Lunar base: starting from every single bolt
* Integration with professional tools: CAD, FEA, Requirements management, MBSE, etc
* Play with friends on your own servers: federated game servers connected via web3 

## Related repositories

* **[Development Guide](/docs/DEVELOPMENT.MD)**
* **[References](/docs/REFERENCES.MD)**
* **[Space Jargon Cheatsheer](https://github.com/LunCoSim/lunco-space-jargon)**
* **[Raw assets](https://github.com/LunCoSim/lunco-assets-raw)**
  
### Design you own mission ###

1. Database of materials (based on db like MAPTIS)
2. Database of component
3. Follow engineering procedures: PDR, CDR, Testing, Integration, Flight, Mission Operation
4. From first unmanned missions to sustainable human colony on the Moon
5. Keep track of your budgets: money, mass, power, data.
6. Thermal, power, communications management
7. Presize interface control description: physical, mechanical, power, data, logical, thermal
8. Supply chain
9. Logistics

## FOLLOW US
1. [Twitter](https://twitter.com/LunCoSim)
2. [Discord](https://discord.gg/emfnjMj3r3)
3. [Youtube](https://www.youtube.com/channel/UCwGFDDQcNSdXA5NxRtNbWYg/videos)
4. [itch.io](https://difint.itch.io/lunco)
5. [Notion](https://www.notion.so/invite/ff7a7dc226d4184c6fb77b1899d6672381be7e44)
6. [Google Drive](https://drive.google.com/drive/folders/1mYNLdYOaw__OIb7OGDZiuHmbZZAJFA7M?usp=sharing)
7. [Reddit](https://www.reddit.com/r/LunCo/)
8. [More information](https://bit.ly/3vNdfKE)
   
## Gameplay
Collect resources, build factories, manufacture robots and to build sustaiable lunar base.

You are playing as a CEO&Founder of a NewSpace Lunar exploration company in the nearest future. 

You want to make the Moon forpost of human civilization in space in order to colonize the whole Solar system.

## What to expect
1. Supply chain management as in Factorio
2. Robot&rocket control as in KSP
3. City management as in SimCity
4. Economics
5. Realistic technologies, physics and enviroment. E.g. you have to extract ~30-50 of different raw resources to build a satellite like Aluminum, Copper.

## How to run
1. Clone this repo
2. Init submodules with command:
    git submodule update --init --recursive 
3. Install [Godot 3.5-rc2](https://downloads.tuxfamily.org/godotengine/3.5-rc2/) for your platform
        [All Godot Versions](https://downloads.tuxfamily.org/godotengine/)
4. Run project by clocking on "project.godot" in "game" folder

## Links

### Like-minded projects
1. [Moonwards](https://www.moonwards.com/) – opensource Lunar City in Godot, lot of assets under MIT
2. [iVoyager](https://www.ivoyager.dev) – a development platform for creating games and educational apps in a realistic solar system, Godot, Apache 2.0
3. [Extraterrestrial Logistics And Space Craft Analogs](https://elascaproject.com/elasca-missions/)
4. [cadCAD](https://cadcad.org) – simulation https://cadcad.org

### References
*PUG* – Payload User Guide

1. [Falcon 9 PUG](https://www.spacex.com/media/falcon-users-guide-2021-09.pdf)
2. [Astrobotic's Peregrine PUG](https://www.astrobotic.com/wp-content/uploads/2022/01/PUGLanders_011222.pdf)
3. [Astrobotic's Cube Rover PUG](https://www.astrobotic.com/wp-content/uploads/2021/07/CubeRover-Payload-Users-Guide-v1.7.pdf)
4. [Intuitive Machines](https://www.intuitivemachines.com/)
5. [iSpace PUG](https://www.mach5lowdown.com/wp-content/uploads/PUG/ispace_PayladUserGuide_v2_202001.pdf)
6. [Masten PUG](https://explorers.larc.nasa.gov/2019APSMEX/MO/pdf_files/Masten%20Lunar%20Delivery%20Service%20Payload%20Users%20Guide%20Rev%201.0%202019.2.4.pdf)
7. [Startship PUG(TBD by SpaceX)]()
8. [FireFly PUG](https://westeastspace.com/wp-content/uploads/2019/08/Firefly-Aerospace-Payload-Users-Guide.pdf)
9. [Virgin](https://virginorbit.com/wp-content/uploads/2020/09/LauncherOne-Service-Guide-August-2020.pdf)

### Standards
1. [NASA-STD-6016 Standard Materials and Processes Requirements for Spacecraft](https://standards.nasa.gov/standard/nasa/nasa-std-6016)
2. [NTRS - NASA Technical Reports Server](https://ntrs.nasa.gov/search)

### Opensource spacecrafts
1. [deathstarinspace](http://deathstarinspace.com)

### Similar games
1. [Kerbal Space Program](https://www.kerbalspaceprogram.com/)
2. [Road to Mars](https://roadtomars.page/)
3. [!Mars](https://marsisflat.space/)
4. [Starbase Simulator](https://ashtorak.itch.io/starbase-simulator)
5. [Spaceport-X](https://www.indiedb.com/games/spaceport-x)
6. [Space Simulator](https://store.steampowered.com/app/529060/Space_Simulator/)
7. [spaceflight-simulator](http://spaceflight-simulator.webflow.io/#videos)
8. [OpenRocket](https://openrocket.info/features.html)
9. [Mars Horizon](https://store.steampowered.com/app/765810/Mars_Horizon/#:~:text=In%20Mars%20Horizon%2C%20you%20take,you%20make%20the%20right%20choices)
10. [Surviving Mars](https://store.steampowered.com/app/464920/Surviving_Mars/)
11. [Children of a Dead Earth](https://store.steampowered.com/app/476530/Children_of_a_Dead_Earth/)
12. [SpaceEngine](https://spaceengine.org/)
13. [Universe Sandbox](https://universesandbox.com/)
14. [Simple Rockets 2](https://www.simplerockets.com)
15. [Planet Base](https://store.steampowered.com/app/403190/Planetbase/)
16. [playfarsite](https://playfarsite.com/l/v1a_t/?f=TW_P1_V_1)

### Professional SW

#### CAD
1. Solidworks
2. FreeCAD
3. Fusion360 
   
#### Thermal
1. Thermal desktop
2. FreeCAD module

#### Structural
1. Inventor
2. Ansys

#### Orbital dynamics
1. GMAT

#### Requirements management and systems engineering
1. IBM Doors
2. JAMA

#### MBSE
1. Arcadia
2. [Innoslate](https://specinnovations.com/capabilities/digital-engineering/)

#### Robotic simulations
1. ROS / Gazebo
2. WeBots
3. MatLab/Simulink

#### Flight frameworks
1. core Flight System (cFS)
2. FPrime
3. ArduPilot

#### Mission Control
1. OpenMCT
2. YAMCS

### Physics simulation ###
1. [mujoco](https://github.com/deepmind/mujoco)
2. [DART](http://dartsim.github.io)

### Databases ###
1. [MAPTIS](https://maptis.nasa.gov)

## Support the project

ETH: 0xA64f2228cceC96076c82abb903021C33859082F8

USDT (ERC-20): 0xA64f2228cceC96076c82abb903021C33859082F8

USDC (ERC-20): 0xA64f2228cceC96076c82abb903021C33859082F8

BTC: bc1qznnpdv4ajq8t5jlyevn7xxdvmkfm8mls3treq0

LTC: ltc1qwtzw9y9hf54mwef6k7htempzmjsqsnrwjxwj2g

DOGE: DJc7Hgw972xXfCM443WYxBfmggRAbeBxq9

TRX: TSGUmrAQpKJHwrS6XHEsYvJn8x6FaK4VzJ

*Created by [DifInt](https://twitter.com/_Difint_)*
