It's possible, however it requires a lot a manual work

1. Setup remote server, ideally ssh access to it, dont forget about protection
2. Ssh into remote server
3. Download godot
```wget https://github.com/godotengine/godot/releases/download/4.1-stable/Godot_v4.1-stable_linux.x86_64.zip
```
4. install unzip
```apt-get install zip unzip
```
5. Unzip godot
```unzip Godot_v4.1-stable_linux.x86_64.zip
```
6. Move godot to make it system wide to 
```mv Godot_v4.1-stable_linux.x86_64 /usr/bin/godot
```
7. Install git-lfs
```apt-get install git-lfs

shell git lfs install
```
8. Clone lunco rep with all submodules
```git clone -b main --single-branch --recurse-submodules https://github.com/LunCoSim/lunco-sim.git
```
9. change directory to LunCo
```cd lunco-sim
```
10. Install addons
```./install_addons.sh
```
11.  Copy .godot folder with cache from local machine to remote using scp
```rsync -avz -e 'ssh -p [remote ssh port]' ./.godot/ [username]@[ip]:[path to lunco-sim]/.godot/

```
12. 