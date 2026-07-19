# Lunar terrain candidates for future scenarios

Research snapshot, 2026-07-19. Verified against the live LROC PDS RDR catalog
(~660 NAC DTM products), PGDA product pages, and the NASA Artemis III region
announcements. This is inspiration/planning material, not a description of
running code.

Earth elevation at a site is fixed by its angular distance from the sub-Earth
point (tidal locking): `elev ≈ 90° − arccos(cos φ · cos λ)`, wobbling ±7–8° with
libration. That single number decides whether a scenario can have a **real**
Earth radio-shadow — the phenomenon the Apollo 15 site cannot produce (Earth
sits ~64° up at Hadley, which is why `traverse.usda` links to a surface mast
instead).

## Findings that change the picture

- **Malapert and Nobile have NAC DTMs** (`NAC_DTM_MALAPERT01–03`,
  `NAC_DTM_NOBILE01–04`): terrain-blocks-Earth gameplay is data-ready at
  2–5 m/px, not a LOLA-only compromise.
- **All three famous pits have dedicated ~2 m/px DTMs**: `NAC_DTM_TRANQPIT1`,
  `NAC_DTM_MARIUSPIT01`, `NAC_DTM_INGENIIPIT`. Caveat: no valid topography
  inside the shadowed pit interiors (interpolated voids) — the pit itself must
  be authored geometry.
- **All four Chang'e sites have NAC DTMs**, including both farside sites —
  permanent Earth blackout (relay-only comms) with zero invented constraints.
- **Apollo 17 has a merged 2 m/px DTM of the whole Taurus-Littrow valley**
  (`NAC_DTM_APOLLO17_2M`) plus 11 sub-DTMs — the best-covered rover valley on
  the Moon, with the real LRV traverses to compare student routes against.
- **Lunokhod 1 has no DTM** (mosaic only — `NAC_ROI_LUNOKOD1LOA`).
  **Lunokhod 2 has three** (`NAC_DTM_LUNOKHOD2`, `_1`, `_2`).
- PGDA hosts **5 m/px LOLA DEMs for named south-pole sites** (Connecting Ridge,
  Shackleton rim, de Gerlache rim, Malapert massif, …) at
  `pgda.gsfc.nasa.gov/data/LOLA_5mpp/` — smooth (no boulders), so pair with NAC
  where possible and say so in the lesson.

## Ranked candidates

Rank = teaching value × data availability. "Earth shadow?" = can terrain
genuinely sever the Earth link.

| # | Site | Coords | Key product (res) | Earth elev | Earth shadow? | Core teaching story |
|---|------|--------|-------------------|-----------|---------------|---------------------|
| 1 | **Malapert Massif** | ~85.7°S, ~2°E | `NAC_DTM_MALAPERT01/02/03` (2–5 m); PGDA Site23 5 m | ~4° | **YES, routine** | Artemis III region; 5 km relief slope ladder; grazing sun; Earth-link relay siting as a lesson |
| 2 | **Shackleton–de Gerlache Connecting Ridge** | ~89.4°S, 137°W | PGDA Site01 5 m; `NAC_DTM_SHACKRDGE02`; `LDEM_80S_20M` | ~0° ±lib | **YES — Earth rises/sets monthly** | Artemis III prime; PSRs metres off-route; illumination-window route timing |
| 3 | **Mare Ingenii pit + swirls** | 35.95°S, 166.06°E | `NAC_DTM_INGENIIPIT` (2 m); `MRINGENII1–6` | −52° | **Permanent blackout** | Farside relay architecture; 130 m skylight hazard; magnetic swirls |
| 4 | **Apollo 17 Taurus-Littrow** | 20.19°N, 30.77°E | `NAC_DTM_APOLLO17_2M` (2 m, whole valley) | ~54° | no (mast) | Nansen, Lee-Lincoln scarp, Sculptured Hills; real LRV traverses; richest heritage |
| 5 | **Nobile Rim** | ~85.2°S, 53.5°E | `NAC_DTM_NOBILE01–04`; `LDEM_80S_20M` | ~3° | **YES** | Artemis III region; NAC-grade polar roughness; PSR-floor descent |
| 6 | **Mare Tranquillitatis pit** | 8.34°N, 33.22°E | `NAC_DTM_TRANQPIT1` (2 m) | ~56° | no | 100 m skylight over a lava tube; flat approach = beginner site with one lethal hazard |
| 7 | **Marius Hills pit + domes** | 14.09°N, 56.77°W | `NAC_DTM_MARIUSPIT01`, `MARIUSDOME1/2`, … | ~32° | no | Skylight + volcanic dome slope ladder + sinuous rilles in one region |
| 8 | **Chang'e-4 / Von Kármán** | 45.46°S, 177.6°E | `NAC_DTM_CHANGE4` | never | **Permanent** | Yutu-2 heritage; relay-satellite gameplay (Queqiao story) |
| 9 | **Shackleton rim** | 89.66°S, 129°E | PGDA Site04 5 m; `LDEM_80S_20M` | ~0° ±lib | **YES, weeks-long** | The iconic PSR; hardest illumination gameplay |
| 10 | **Lunokhod 2 / Le Monnier** | 25.85°N, 30.45°E | `NAC_DTM_LUNOKHOD2`, `_1`, `_2` | ~51° | no | Retrace the 39 km record traverse; Fossa Recta rille; Soviet heritage |
| 11 | **Tycho central peak** | 43.31°S, 11.36°W | `NAC_DTM_TYCHOPK`, `PK01–08` | ~45° | no | 2 km peak climb, melt ponds, boulder fields, the 120 m summit boulder |
| 12 | **Vallis Schröteri / Aristarchus** | 24.5°N, 49°W | `NAC_DTM_VSCHROTERI(2)`, `ARISTPLAT1` | ~37° | no | Largest sinuous rille (1 km deep) — canyon LOS comms with a mast |
| 13 | **Chang'e-6 / Apollo basin** | 41.64°S, 154°W | `NAC_DTM_CHANGE6` | never | **Permanent** | First farside sample return (2024); current-events hook |
| 14 | **Chandrayaan-3 / Vikram site** | 69.37°S, 32.32°E | `NAC_DTM_VIKRAMSITE1` | ~17° | no | Pragyan heritage; long shadows without full polar difficulty |
| 15 | **Ina D-caldera** | 18.66°N, 5.3°E | `NAC_DTM_INACALDER2M` (2 m) | ~70° | no | Enigmatic young volcanics; delicate-terrain science traverse; 2×3 km = perfect crop |
| 16 | **Reiner Gamma swirl** | 7.5°N, 59°W | `NAC_DTM_REINER_2M` (2 m) | ~31° | no | Magnetic-swirl science (Lunar Vertex target); flat driving-school site |
| 17 | **Hyginus rille + crater** | 7.8°N, 6.3°E | `NAC_DTM_HYGINUS` | ~80° | no | Rimless volcanic collapse crater + rille chain; graben teaching |
| 18 | **Schrödinger basin** | ~75°S, 132.4°E | `NAC_DTM_SCHRODNGR01–03`, `SCHRODVENT1–4` | −10° | **Permanent** | Farside + polar-ish; volcanic vent in a peak-ring basin |

Notable non-candidates: **Lunokhod 1** (no DTM) · **Rima Ariadaeus** (no DTM —
use Hyginus) · **Mons Mouton** (no dedicated NAC DTM found; PGDA/LDEM_80S only)
· **Compton-Belkovich** (61.1°N 99.5°E — Earth at −5° mean: *libration-
intermittent* Earth visibility, unique but slow-timescale) · **Gruithuisen
domes** (`NAC_DTM_GRUITHUI_2M` exists, Lunar-VISE target, but no comms story).

## A progression arc across scenarios

1. **Hadley (current)** — slope + mast shadow. The taught case.
2. **Apollo 17 / Lunokhod 2** — heritage re-enactment: drive where they drove,
   compare your route to the real one. Same mechanics as Hadley, richer story.
3. **Tranquillitatis pit** — one lethal hazard on an easy plain; sensor caution.
4. **Malapert / Nobile** — the real thing the mast stands in for: terrain
   severs the Earth link, and a QGIS viewshed from Earth's az/el *predicts* the
   blackout zone before the drive verifies it. Grazing sun makes the shadow map
   load-bearing too.
5. **Farside (Chang'e-4 / Ingenii)** — Earth never visible; relay masts or an
   orbiter (`KeplerOrbit` link endpoints exist) become the mission.
6. **Connecting Ridge / Shackleton** — endgame: illumination windows, PSRs,
   monthly Earth rise/set. Every hazard class interacting.

## Pipeline gates before the best sites work

| Gate | Blocks | Note |
|---|---|---|
| **Polar stereographic GeoTIFF** | #1, #2, #5, #9, #18 | `lunco-geotiff` currently assumes equirectangular-at-crop-centre; polar NAC DTMs and all PGDA polar products ship polar stereo |
| Pit interior voids | #3, #6, #7 | DTMs have interpolated floors inside shadowed pits; author the pit as geometry |
| Real Earth az/el (libration) | #2, #9 (and honesty at #1, #5) | Near 0° elevation, ±8° libration decides whether Earth is up at all; the ephemeris subsystem already knows where Earth is — use it rather than authoring a constant |
| LOLA smoothness | #2, #9 | 5 m LOLA products carry no boulder-scale roughness; either accept (and say so) or add the overzoom layer honestly labelled synthetic |

## Data products cheat sheet

| Catalog | What | Res | URL |
|---|---|---|---|
| LROC NAC DTM RDR | ~660 stereo DTM sites, float32 GeoTIFF + ortho + slope | 2–5 m/px | `data.lroc.im-ldi.com/lroc/view_rdr/NAC_DTM_<NAME>` · PDS dir: `pds.lroc.im-ldi.com/data/LRO-L-LROC-5-RDR-V1.0/LROLRC_2001/DATA/SDP/NAC_DTM/` |
| NAC DTM footprints | Coverage shapefile for QGIS site scouting | — | `data.lroc.im-ldi.com/lroc/view_rdr/SHAPEFILE_NAC_DTMS` |
| PGDA LOLA site DEMs | Named south-pole sites, LDEM + slope + uncertainty | 5 m/px | `pgda.gsfc.nasa.gov/data/LOLA_5mpp/` |
| PGDA south-pole mosaics | `LDEM_80S_20M` class (10 m variants) | 10–20 m/px | `pgda.gsfc.nasa.gov/products/81`, newer `/products/90` |
| SLDEM2015 | ±60° lat global (LOLA+Kaguya) | ~59 m/px | `imbrium.mit.edu` |
| Kaguya TC | Near-global stereo fallback | ~10 m/px | JAXA SELENE archive |
| USGS Astropedia | Landing-site orthomosaics for textures (Apollo 17 at 50 cm) | 0.5–2 m/px | `astrogeology.usgs.gov/search` |
