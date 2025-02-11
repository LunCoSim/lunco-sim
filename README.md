# LunCo: Everyone can do Space

LunCo is a open-source simulation tool designed for planning space missions, with a focus on lunar settlements. 

![](https://gateway.lighthouse.storage/ipfs/bafybeibtwxdybz5onr5zwqotia64lbsgju6r55nwp23bosd4mxwy25siqa)
*Coming soon via [ivoyager](https://github.com/ivoyager)*

Built with the powerful Godot 4 engine, LunCo aims to revolutionize the way space engineers design and collaborate on complex systems.

## [Try in Browser](https://alpha.lunco.space)


## 🌌 Vision

Our vision is to provide a comprehensive suite of open-source applications tailored for Lunar Base engineering, including:

- **Unified Platform**: LunCo serves as a central hub, bringing together the best of open-source space engineering tools, offering a unified experience for users.
- **Requirements Management**: Streamline and manage your project requirements with ease.
- **Models Visualizations**: Visualize and interact with your designs in a 3D environment.
- **Collaborative Training**: Train and collaborate with your team in real-time.
- **Digital Twin**: Create a digital replica of your lunar colony for simulations and analysis.

![](https://gateway.lighthouse.storage/ipfs/bafybeidjpafb6zg5lalug7z5sfzvszh2erskbbdqcloejr2asex2lfg4ky)


## 🛠 Installation

0. The development is done on Linux Mate, so there could be issues running on Windows and MacOs. Please reach us

1. Install [Godot 4.4 Beta 2](https://godotengine.org/article/dev-snapshot-godot-4-4-beta-2/#downloads)

2. Install [git lfs](https://github.com/git-lfs/git-lfs#getting-started). It handles large files in the repository.

3. Clone this repo in a terminal: 
```bash
git clone -b main --single-branch --recurse-submodules https://github.com/LunCoSim/lunco-sim.git
```

4. After cloning, change directory to project folder
```bash
cd lunco-sim
```

5. Enable git-lfs in the repository after cloning: 
```bash
git lfs install
```

6. We still need to download the content files from git-lfs. Run the following command to download them:
```bash
git pull --recurse-submodules
git lfs pull && git submodule foreach git lfs pull
```

7. Now open project and wait till intenal conent management downloads all the files. LunCo Content Manager (new system, gradually being adopted):
   1. Will be installed automatically with other addons
   2. After installation, you'll see a "Content" button in the editor toolbar
   3. Use it to download missing content files when needed

8. Wait till all the files are downloaded. You'll see the message in the Output tab.

9. Restart editor and enjoy!


### Content Management Notes
- Some large files are still managed by git-lfs
- Newer content will use `.content` files for external storage
- If you see missing files:
  1. First try git-lfs: `git lfs pull`
  2. Then use the Content Manager in the editor toolbar
  3. If issues persist, please reach out on Discord

## 💰 Donations

Support us on [JuiceBox](https://juicebox.money/v2/p/763)!


## 🚀 Features

1. **Lunar 3D Mapping**: Dive into a high-resolution 3D map of the Moon, offering unparalleled detail and accuracy. Plan your missions with precision, leveraging real lunar spatial data for an immersive experience.
	
2. **Collaborative Mission Design**: Work in real-time with fellow space engineers from around the world. Share, discuss, and refine your lunar mission designs in a collaborative metaverse, powered by web3 tools.
	
3. **IP-NFT for Designs**: Protect and monetize your innovative space mission designs by issuing them as Intellectual Property Non-Fungible Tokens (IP-NFTs). Showcase your expertise and gain recognition in the space engineering community.
	
4. **Decentralized Engineer Profiles**: Create your decentralized engineer profile. Manage access, showcase your projects, and connect with peers in a secure and transparent manner.
	
5. **Interactive Training Modules**: Engage in hands-on training sessions within the LunCo platform. Simulate real-world lunar scenarios, test your designs, and receive instant feedback, all within a dynamic and interactive environment.


## 🌐 Community & Support

Join our vibrant community and stay updated on the latest developments:

- [Discord Server](https://discord.gg/uTEFrW32)
- [Twitter](https://twitter.com/LunCoSim)
- [Official Website](https://lunco.space/)
- [LinkedIn](https://www.linkedin.com/company/luncosim/)
- [YouTube Channel](https://www.youtube.com/@LunCoSim)

## 💖 Support LunCo

Support us on [JuiceBox](https://juicebox.money/v2/p/763)!
