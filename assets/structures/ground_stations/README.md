# Ground stations

Real deep-space facilities, one file per dish. Each is a **thin instance** of
`components/comms/ground_station.usda`: it names a place, pins it to a geodetic
point, and sets its aperture. Nothing else.

```usda
def "BearLakes" ( prepend references = @lunco://structures/ground_stations/bear_lakes.usda@ ) {}
```

That is the whole authoring surface a scene needs. Do **not** reference
`ground_station.usda` directly from a scene and paste coordinates next to it —
that is what this folder exists to stop, and it was previously duplicated across
every traverse scene in the Space School twin.

## The catalogue

| File | Facility | Dish | lat, lon | Network |
|---|---|---|---|---|
| `dss_madrid.usda` | Robledo de Chavela, Spain | DSS-63, 70 m | 40.4314, −4.2481 | NASA DSN |
| `dss_goldstone.usda` | Goldstone, Mojave, USA | DSS-14, 70 m | 35.4267, −116.89 | NASA DSN |
| `dss_canberra.usda` | Tidbinbilla, Australia | DSS-43, 70 m | −35.4014, 148.9817 | NASA DSN |
| `bear_lakes.usda` | Медвежьи Озёра, Moscow obl. | TNA-1500, 64 m | 55.87, 38.23 | RU deep space |
| `ussuriysk.usda` | Галёнки, Primorsky Krai | P-2500 / RT-70, 70 m | 44.02, 131.76 | RU deep space |
| `kalyazin.usda` | Калязин, Tver obl. | RT-64, 64 m | 57.22, 37.90 | RU, VLBI / backup |

Bear Lakes + Ussuriysk are the pair that flew **Luna-25** (2023), the most recent
Russian lunar mission. They sit ~93° of longitude apart, which is what buys most
of a day's lunar coverage from two sites instead of the DSN's three.

Longitude is **east-positive**, matching `lunco:anchor:lon` everywhere else in
the tree (`wrap_lon_deg` keeps the antimeridian honest).

## Accuracy, stated plainly

Latitude and longitude are **site-level** — they land on the right facility, not
on the right pier. Heights for the Russian stations are **approximate ground
elevations**, not surveyed antenna reference points.

This is fine for what the numbers are used for: the link kernel computes range,
elevation and occultation against a 384,400 km baseline, where a 500 m siting
error is 1.3 µrad. It is **not** fine if someone later wants real Doppler,
ranging residuals or VLBI baselines — those need ITRF station coordinates, and
this table would have to be replaced wholesale rather than nudged.

## What a station file may contain

Four anchor values, a `lunco:label`, and one `DishHead` scale. Aperture =
`1.16 · s` metres, so 70 m is `s = 60.34` and 64 m is `s = 55.17`. Anything else
— pedestal, reflector shape, elevation mask, billboard behaviour, the fact that
the dish does not slew — belongs in `components/comms/ground_station.usda` and
applies to every station at once. If you find yourself overriding geometry here,
the generic host is wrong and should be fixed there.

## Seeing them on the globe

A 70 m dish on a true-scale Earth (6371 km radius) is ~1/91,000 of the body:
**sub-pixel from any camera that can see the whole globe.** The geometry is
correct and physically sized, and it is invisible at that range — that is not a
bug to be fixed by inflating the model.

What marks the site at globe distance is the **billboard**: the generic host
authors `lunco:billboard` with a `{label}\n{lat}, {lon}` template and a 50,000 km
`fadeEnd`. That covers every sane globe-view camera distance (Earth auto-focus
sits near 3× radius ≈ 19,000 km) while staying under the 384,400 km lunar
distance, so the six stations do not stack their labels onto the Earth disc in
the lunar sky.

A screen-constant-size **marker pin** — geometry that subtends a fixed angle
regardless of range, so a station reads as a shape and not only as text — does
not exist in the engine today. The shape it would take is the same one
`link_beam.usda` already uses: a `lunco:placeholder` prim plus a
`LunCoProgramAPI` driver that rewrites its scale each frame from camera
distance. That is a general capability (waypoints, POIs, orbiting vessels all
want it), not a ground-station feature, so it does not belong in this folder.
