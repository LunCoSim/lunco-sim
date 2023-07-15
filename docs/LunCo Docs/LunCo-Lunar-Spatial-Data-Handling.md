# Lunar Spatial Data Handling
***Isn't it cool to walk on the Moon? Even in a simulation***

## Expected outcome

Being able to click on any location on the moon and have a decent 3D model of the location to traverse it using existing operators


## Problem

High-quality 3D map of the Moon is crucial for mission planning. Such a map should be able to include several layers, like Rocks distribution, Dust dencity, etc. 
Visualisation of mission location, and possibility to quickly try different ways to travers it would cool. 
Planet-scale data storage is a solved problem with tons of existing solutions, howevert it's still a very trick task.
There are tons of open data sources, so ideally the whole pipilene of data merging to be implemented in an easy-to config way.
[NASA Moon CGI Kit ](https://svs.gsfc.nasa.gov/cgi-bin/details.cgi?aid=4720)is a nice starting point. It has:
- The color map: 24-bit RGB TIFF, max 27360x13680 pix (494.1 MB)
- Height map: 23040x11520 pix (1012.6 MB)  64 ppd (pixel per degree)  floating-point TIFFs in kilometers, relative to a radius of 1737.4 km



## Solution

There is a an interesting [MTerrain](https://github.com/mohsenph69/Godot-MTerrain-plugin) plugin designed to handle openworld games. For the lunar environment there could be a planetary level LOD generated, when at big distances it's just a texture.

H(lat, lon) - heigh of the terration at lat, log

### Questions
1. Chunk size should be tuned to MTerrain, e.g. 100x100 meter
2. How H relates to Height map, conversion function?
3. What 64 ppd means? It means that for each degree there are 8 measures, so spatial resoultion is 1/8 of the degree? 