# Nightglow Color Variation: Research Notes

**Date:** 2026-03-22
**Purpose:** Understand why nightglow (airglow) shifts between orange, yellow, green, and white in ISS timelapses, and what variables drive those color changes.

---

## Executive Summary

Nightglow is not a single emission: it is the sum of at least five distinct chemiluminescent processes occurring at different altitudes in the 75–300 km range. Each produces a different color. From the ISS looking at the limb, the layers are physically separated by altitude, but they overlap in the line-of-sight integration, so the observed color at any moment is a mixture whose dominant hue depends on which layer is currently brightest. The dominant emitters in the visible range are:

- **Atomic oxygen O(1S) at ~95 km** — green, 557.7 nm
- **Sodium (Na) at ~87–92 km** — yellow-orange, 589 nm (D lines)
- **FeO "orange arc" at ~87 km** — orange broadband continuum, peaks ~595 nm
- **OH Meinel bands at ~87 km** — red-to-near-IR, faint visible tail
- **Atomic oxygen O(1D) at ~200–300 km** — red, 630 nm (faint, often not visible in photos)

The color shifts in ISS timelapses are primarily caused by: (1) the physical vertical separation of these layers becoming visible as the ISS changes viewing angle relative to the limb, (2) real intensity variations driven by latitude, local time, season, and solar cycle, and (3) camera exposure and sensor response mixing or saturating the faint signal.

---

## 1. Emission Lines and Their Altitudes

### Green: OI 557.7 nm — mesopause, ~95 km

The green line is produced when atomic oxygen in the excited O(1S) state transitions to the lower O(1D) state, emitting a photon at 557.7 nm. The reaction chain is:

```
O + O + M -> O2* + M     (three-body recombination)
O2* + O -> O(1S) + O2    (Barth mechanism, energy transfer)
O(1S) -> O(1D) + hν(557.7 nm)
```

The peak emission altitude is **92–105 km** in the mesosphere/lower thermosphere (MLT), with typical peak around 95–97 km. A secondary, much weaker peak exists at 140–180 km in the thermospheric F-region from dissociative recombination of O2+ with electrons, but this is normally negligible in the nightglow budget.

The layer is narrow: approximately 6–10 km full width at half maximum (FWHM), centered near the mesopause.

The green line is "one of the brightest [visible] emissions on Earth" and its wavelength falls where the human eye and most camera sensors are most sensitive, which is why it dominates photographs.

### Orange/Yellow: Sodium D lines, 589 nm — mesopause, ~87–92 km

Sodium is deposited in the upper mesosphere by meteoric ablation and forms a diffuse layer from about 80 to 110 km, peaking near 90 km. At night, sodium atoms react with ozone:

```
Na + O3 -> NaO + O2
NaO + O -> Na(3P) + O2   (or Na* + O2)
Na(3P) -> Na(3S) + hν(589.0/589.6 nm)   (D2/D1 doublet)
```

The result is the well-known sodium D-line doublet at 589.0 and 589.6 nm, which appears yellow-orange to the eye and to cameras.

The sodium layer altitude (~87–92 km) is slightly **below** the green oxygen layer (~95–97 km). In ISS limb photographs, this creates a vertical color separation: orange/yellow below, green above. This is the primary source of the "orange lower band, green upper band" structure seen in many ISS images.

The sodium layer also shows a sporadic enhancement phenomenon where isolated pockets of sodium can be much denser than the background, temporarily brightening the orange emission.

### Orange Continuum: FeO "Orange Arc" Bands — ~85–90 km

Iron deposited by meteorites reacts with atmospheric ozone to produce excited iron oxide:

```
Fe + O3 -> FeO* + O2
```

This produces a quasi-continuum emission spanning roughly **540 to 680 nm**, peaking near **595 nm** — solidly orange. The FeO emission was only definitively identified in airglow spectra using the OSIRIS spectrograph on the Odin satellite. Its altitude profile closely matches the sodium and OH layers (~87 km), not surprising since they share a common meteoric source and all react with O3.

FeO contributes a modest but real broadband orange signal that adds to the sodium D-line yellow-orange. The continuum nature of FeO emission (vs. the discrete sodium doublet) means that it cannot be cleanly separated from sodium in low-resolution photographs, and the two together produce the characteristic warm orange/yellow color of the lower airglow band.

### Red: OI 630.0 nm — thermosphere, ~200–300 km

The red airglow line comes from atomic oxygen in the O(1D) state de-exciting to ground state. The O(1D) atoms are produced high in the F-region of the ionosphere (~200–300 km) primarily through:

```
O+ + O2 -> O2+ + O
O2+ + e- -> O(1D) + O    (dissociative recombination)
```

The 630 nm emission is fundamentally an **ionospheric** phenomenon, not a mesospheric one. It sits roughly 100–200 km above the green and orange layers. The O(1D) state has a long radiative lifetime (~110 seconds), which means it is easily quenched by collisions in the denser lower atmosphere — this is precisely why the emission only survives at high thermospheric altitudes where the atmosphere is thin enough that the excited atom can emit before being collisionally deactivated.

In ISS limb photos, the red/orange layer from 630 nm is sometimes visible as an upper reddish tinge **above** the green layer, though it is substantially weaker than the green in absolute brightness and is easier to observe photometrically than visually. It is not the same as the sodium orange — the 630 nm red sits far higher (200+ km) while the sodium orange sits at 87–92 km.

### OH Meinel Bands — ~83–87 km, near-IR with visible tail

The hydroxyl radical (OH) is the single strongest nightglow emitter by total photon flux, but its emission is concentrated in the **near-infrared** (vibrational transitions from ~600 nm to ~4 μm). The strongest bands are in the 1–4 μm range, well outside visible. However, higher-energy bands (e.g., Meinel (8,3) and (9,3)) extend into the 620–750 nm visible red range.

The OH layer peaks at **~87 km** with a narrow FWHM of ~7 km — essentially collocated with the sodium and FeO layers. In a broadband camera, OH emission in the 620–750 nm range contributes a faint reddish tinge that blends with the sodium orange, making the lower airglow band appear somewhat redder-orange rather than purely yellow.

OH emission intensity varies by more than an order of magnitude irregularly, making it one of the harder components to predict.

### Summary Table

| Emitter | Wavelength | Color | Altitude (peak) | FWHM |
|---------|-----------|-------|-----------------|------|
| O(1S) green line | 557.7 nm | Green | ~95–97 km | ~8 km |
| Na D lines | 589.0/589.6 nm | Yellow-orange | ~87–92 km | ~10 km |
| FeO orange arc | 540–680 nm (peak ~595 nm) | Orange | ~85–90 km | ~8 km |
| OH Meinel visible tail | 620–750 nm | Red-orange | ~83–87 km | ~7 km |
| O2 atmospheric band | 762 nm (and 865 nm) | Deep red/near-IR | ~95 km | ~8 km |
| O(1D) red line | 630.0 nm | Red | ~200–300 km | broad |

---

## 2. Vertical Layering as Seen from the ISS Limb

From the ISS at ~400 km altitude, looking tangentially at the limb, the airglow layers are **physically separable** because the line of sight passes through each layer at a different apparent height above the horizon. A NASA Earth Observatory image (October 7, 2018, Expedition 57) explicitly shows:

- A lower orange-yellow band: sodium (589 nm) + FeO continuum, at 85–92 km
- An upper green band: atomic oxygen (557.7 nm), at 95–97 km

This separation of roughly 5–10 km in altitude translates to a visible spatial gap in the limb view. When the ISS camera is aimed slightly differently — more toward the zenith versus more horizontal — or as the ISS orbital altitude changes the geometry, the relative prominence of the two layers shifts. This geometry effect alone explains much of the apparent color "shifting" in a timelapse as the ISS moves.

The red O(1D) line at 200–300 km can sometimes appear as a faint red fringe above the green layer, visible in some ISS photographs as a "red at the top" feature. NASA's SVS documentation of ISS airglow sequences specifically notes this: "Red airglow shows up about halfway through the video" in one Expedition 57 sequence.

So in an ISS timelapse, you might observe bottom-to-top: orange/yellow (sodium + FeO), then green (atomic oxygen), then faint red (O(1D) thermosphere). The exact proportions depend on viewing geometry and real intensity variations.

---

## 3. Variables Affecting Color

### Geographic Latitude

Latitude is a major driver of which emission dominates.

**Equatorial region (roughly ±20° latitude):** The OI 630 nm red line is dramatically enhanced near the equatorial ionization anomaly (EIA). Post-sunset, thermospheric O+ drifts upward along field lines, enhancing O2+ recombination on the flanks of the magnetic equator at roughly ±15°–30° latitude. This creates characteristic paired bright arcs in 630 nm imaging, so photos taken over equatorial regions can appear more reddish-orange in the upper layer than photos taken over mid-latitudes.

Also in the equatorial region, equatorial plasma bubbles (EPBs) appear as dark voids in the 630 nm emission — depleted regions where the ionosphere has undergone Rayleigh-Taylor instability. These are visible from the ISS as dark streaks or bands superimposed on the airglow.

**Mid-latitudes:** More dominated by the mesospheric emissions (green and sodium orange) which are relatively uniform. The 630 nm thermospheric red is weaker here.

**High latitudes / polar regions:** Auroral contamination becomes significant. The aurora shares some of the same emission lines (OI 557.7 nm green, OI 630 nm red), so at high latitudes the "airglow" in an ISS image can include auroral contributions, often appearing brighter and more structured than pure airglow.

### Season and Time of Year

Each emission layer has its own seasonal pattern:

- **O(1S) green (557.7 nm):** Shows correlation with atmospheric circulation patterns that transport atomic oxygen. Studies of solar cycle 23 show the average intensity peaks at ~300–400 Rayleighs in February/March in the northern hemisphere winter. The seasonal variation is primarily driven by changes in atomic oxygen concentration in the MLT, which in turn is modulated by the large-scale mesospheric circulation (the mesosphere is coldest and has the highest atomic oxygen density at the winter pole due to descent from above).

- **Sodium (589 nm):** Shows a semi-annual cycle in the tropics with peaks around the spring and fall equinoxes, driven by the semi-annual variation of the diurnal tidal amplitude. At winter solstice the peak emission altitude shifts upward by ~5 km compared to summer solstice. Sodium does not closely follow the solar activity cycle (unlike O(1S)).

- **O2 atmospheric band (762 nm):** High intensities occur mainly in April at many stations.

- **OH bands (~87 km):** Most intense during southern mid-winter (June/July in the southern hemisphere) and least intense in summer. Vary by more than an order of magnitude. High OH intensity correlates with lower mesopause temperatures (OH intensity is temperature-sensitive).

The practical implication: an ISS timelapse filmed in boreal winter over the northern hemisphere will show stronger green relative to the sodium yellow than the same pass in boreal summer, when sodium is relatively more prominent and the green is weaker.

### Time of Night

There is a well-documented diurnal pattern in nightglow emissions that causes color shifts through the night.

In the early evening, shortly after sunset at mesospheric altitudes (which occurs later than surface sunset because those altitudes can still see the sun when the ground cannot), the atmosphere is still photochemically active. The **sodium** and **OI 630 nm** emissions are relatively enhanced post-sunset. One source explicitly states: "After the Sun has also set for these altitudes at the end of nautical twilight, the intensity of light emanating from the sodium lines and red lines decreases, until the oxygen-green remains as the dominant source."

This describes a real color shift: in the hours just after astronomical twilight, the nightglow has a more yellow-orange character. As the night progresses toward midnight and beyond, the green OI 557.7 nm emission becomes relatively more dominant. So an ISS timelapse crossing from just-post-sunset regions to deep-night regions of Earth would show the airglow transitioning from yellow-orange toward greener.

The OI 630 nm red line also shows a "post-midnight enhancement" at low latitudes, a thermospheric phenomenon related to the midnight temperature maximum. Near dawn, photochemical processes begin ramping up again as the sun approaches the high-altitude horizon.

### Solar Activity and the Solar Cycle

The 11-year solar cycle affects the emission lines differently:

- **OI 557.7 nm green:** Strong positive correlation with solar flux (F10.7 index). During solar maximum, the average intensity reaches ~300–400 Rayleighs (roughly 20% higher than solar minimum). The mechanism is increased EUV/UV photodissociation of O2 and O3, generating more atomic oxygen available for the green-line reactions. The green line is thus measurably brighter during solar maximum years.

- **OI 630 nm red:** Also correlated with solar activity, because it depends on O+ ion density which is driven by solar EUV ionization.

- **Sodium (589 nm):** Notably, sodium nightglow does NOT closely follow the solar cycle. Its variability is dominated by mesospheric dynamics (tides, gravity waves), not solar EUV directly.

- **OH bands:** The OH intensity follows a complex mix of solar cycle and dynamical effects.

The practical consequence: the ratio of green (solar-cycle-sensitive) to orange (less solar-sensitive) shifts over the 11-year cycle. Near solar maximum, nightglow looks greener relative to a solar minimum year.

### Geomagnetic Activity and Auroral Contamination

At mid-to-high latitudes (roughly poleward of ±50°), geomagnetic storms cause the auroral oval to expand equatorward, bringing genuine aurora to latitudes normally showing pure airglow. Aurora uses many of the same emission lines — OI 557.7 nm (green), OI 630 nm (red), N2+ (blue, 391–428 nm) — so in a widefield ISS photograph the auroral and airglow contributions blur together.

During strong storms (G3 or higher), "Stable Auroral Red" (SAR) arcs can appear at mid-latitudes as diffuse red bands, generated by energy deposition from the ring current into the upper atmosphere. These are chemically similar to the thermospheric 630 nm airglow but magnetically driven. An ISS pass during a geomagnetic storm over, say, 50° latitude could show dramatically redder-than-normal airglow because of this contamination.

At purely equatorial latitudes, geomagnetic contamination is minimal, and the airglow is "clean" chemiluminescence.

---

## 4. ISS Photographer and NASA Observations

NASA and ISS crews have explicitly documented the multi-color character of airglow:

- **Expedition 57 (October 2018):** A 26-minute sequence over Bangladesh, Australia, and New Zealand shows the green layer first, then "red airglow shows up about halfway through the video." NASA Earth Observatory described the composition in terms of green (oxygen recombination) and orange (sodium) as distinct colors.

- **Expedition 28 (~2011):** Widely cited as "possibly the first really good airglow image from Earth orbit which even allows to distinguish different colours, according to height in the mesopause region." This was the image that made the layered orange/green structure publicly visible and widely reproduced.

- **NASA SVS documentation (ID 12963):** Explicitly lists observations showing "red at the top," "green below the red," and "orange" hovering over Earth's curve in separate sequences, confirming that all three colors are seen in different frames of a single timelapse depending on geometry and geographic position.

- **Indian Ocean airglow (July 2020):** An astronaut photograph from this pass is the most reproduced NASA airglow image, showing a clear orange-yellow lower band (sodium + FeO) with a green upper band (atomic oxygen), and a diffuse star field above. NASA's caption explicitly attributes orange to sodium at 85–90 km and green to atomic oxygen at 95–100 km.

- **Astronaut quote (Christina Koch and others):** ISS crew members have described the airglow band as visually faint — it is not naked-eye-bright from inside the ISS under normal lighting conditions, but is obvious in long-exposure photographs. The band appears as a thin colored arc along the limb, not a diffuse haze filling the whole sky.

- **Aurora/Airglow overlap (March 2020):** An ISS photograph taken "just before dawn as the ISS passed south of the Alaskan Peninsula" shows "wavy green, red-topped wisps of aurora" intersecting a "muted red-yellow band of airglow," demonstrating the two phenomena co-occurring and creating compound color structure.

---

## 5. The "White" Component

Observation: ISS timelapses sometimes show the airglow as nearly white or colorless. Several mechanisms can explain this:

### Overlap of multiple emission lines

At the transition zone between the orange and green layers, the line-of-sight integration can simultaneously capture significant flux from sodium (589 nm) and green oxygen (557.7 nm). These two wavelengths are roughly 32 nm apart in the yellow-green region. A camera sensor registering both simultaneously would desaturate toward a yellow-white, since yellow-green + orange yields a warm white in sRGB color space. This "white zone" would appear at intermediate altitudes between the two layers, and may be the most common cause of white appearances in photographs.

### FeO orange continuum + OH visible tail

The FeO emission spans 540–680 nm with a flat-ish continuum, and OH bands contribute in the 620–750 nm range. If a camera has a broadly responsive sensor and these continua are well-lit, the combined spectrum from ~540 to ~750 nm (without strong peaks in any one color) will render as yellow-white or near-white rather than a specific hue.

### Camera overexposure / sensor saturation

In ISS timelapses, exposure times are typically 2–4 seconds at ISO 51200–102400 on Nikon D3s/D4/D5 cameras. Regions where the airglow is brighter than normal (e.g., near the magnetic equatorial arcs in 630 nm emission, or during gravity wave intensity peaks) can saturate the camera sensor, driving all color channels to maximum and producing a white appearance.

### Nightglow broadband continuum (FeO + HO2)

A 2024 study in Atmospheric Chemistry and Physics (Noll et al., ACP 24, 1143) identified a genuine broadband continuum in nightglow from 300 to 1800 nm. In the visible range (500–720 nm), this contributes approximately 5 Rayleighs per nanometer as a near-flat continuum. The dominant visible contributor is the FeO "orange arc" (peaking 595 nm, ~3% of total emission in the 500–720 nm range) plus a newly identified HO2 near-IR component. This continuum alone is too faint to appear white in normal ISS exposures, but in combination with other emissions could contribute a broadening and whitening of the airglow spectrum.

### Blue/UV channel contribution

Atomic oxygen and nitrogen also produce emissions in the blue-UV range. NASA documentation mentions that "other reactions can produce blue, UV, and infrared light." If any blue emission is simultaneously bright enough to register alongside the orange and green, the RGB result moves toward white.

**Most likely explanation for observed white in timelapse:** A combination of (1) the spatial zone where orange and green layers overlap in the line-of-sight integral, (2) moderate overexposure causing channel saturation, and (3) camera auto-white-balance or post-processing color grading that normalizes the airglow to white.

---

## 6. Practical Summary for a Renderer

### The most important variables

If implementing a physically-motivated airglow layer in a 3D Earth renderer, the variables ordered by importance are:

**1. Viewing geometry (limb angle) — dominates color**
The single biggest driver of color in ISS images is the viewing angle through the airglow layers. Looking tangentially along the limb, orange (87–92 km) and green (95–97 km) are spatially separated. At a steeper viewing angle (looking down through the atmosphere), the layers integrate together, blending toward yellow-white. A renderer must model this correctly to reproduce the stratified color structure.

**2. Geographic latitude — controls which emission is enhanced**
- Equatorial (±20°): OI 630 nm (red) is enhanced post-sunset via the EIA; equatorial plasma bubbles create dark voids; overall warmer-toned airglow
- Mid-latitudes (±20–50°): Cleanest, most typical green/orange layering
- High latitudes (±50+°): Auroral contamination possible; would need a separate aurora model

A latitude-dependent blend factor between "warm orange-dominated" (equatorial) and "pure green" (mid-latitude) would capture the main geographic variation.

**3. Time of night (local solar time) — controls orange/green balance**
- Just after astronomical twilight: yellow-orange relatively enhanced (sodium + OI 630 nm post-sunset enhancement)
- Midnight: green becomes most dominant, orange weaker
- Pre-dawn: emissions begin to ramp back up

Approximation: a smooth sigmoid or sinusoidal modulation of the orange/green ratio through local time, from ~60% orange at dusk, to ~30% orange at midnight, back to ~50% orange at pre-dawn.

**4. Solar cycle phase — modulates green intensity by ~20%**
Not perceptible in a real-time renderer unless you want historical accuracy. Could be a single scalar multiplier on the green channel keyed to user-set date (F10.7 proxy).

**5. Season — minor secondary variation**
The most notable seasonal effect visible in photographs is OH intensity (which contributes to the warm-red lower band): highest in mid-winter, lowest in mid-summer at a given hemisphere. This is a ~50% variation but is dominated by geometry effects in most viewing conditions.

### Simple approximation

For a real-time renderer, a workable approximation would be:

- Render the airglow as two altitude layers: "lower" (87–92 km) and "upper" (95–97 km).
- Lower layer color: warm orange-yellow (roughly sRGB 1.0, 0.7, 0.2), modulated by local time (stronger post-sunset) and latitude (brighter near equatorial flanks ±20°).
- Upper layer color: pure green (roughly sRGB 0.2, 1.0, 0.1), modulated by solar cycle and season but relatively stable.
- The "white" seen in some images falls out naturally from the additive blending of both layers at intermediate viewing angles.
- Gravity wave structures (the wavy ripples in the airglow) add visually interesting texture but require a noise function applied to layer brightness — they do not change the fundamental colors.

This is not fully physically accurate but will produce the qualitatively correct color variation as the viewer's latitude and time-of-night position change.

---

## Sources

- [Airglow — Wikipedia](https://en.wikipedia.org/wiki/Airglow) — comprehensive overview of emission mechanisms, altitudes, and colors
- [Why NASA Watches Airglow — NASA](https://www.nasa.gov/solar-system/why-nasa-watches-airglow-the-colors-of-the-upper-atmospheric-wind/) — observational context from NASA Goddard, ISS color descriptions
- [Airglow Over the Indian Ocean — NASA Earth Observatory / NASA Science](https://science.nasa.gov/earth/earth-observatory/airglow-over-the-indian-ocean-147367/) — key ISS photograph with explicit attribution of orange to sodium (85–90 km) and green to oxygen (95–100 km)
- [Aurora, Meet Airglow — NASA Science](https://science.nasa.gov/earth/earth-observatory/aurora-meet-airglow-147122/) — ISS photograph showing airglow and aurora coexisting, muted red-yellow band described
- [Earth Awash in Lights of the Night — NASA Science](https://science.nasa.gov/earth/earth-observatory/earth-awash-in-lights-of-the-night-92912/) — Expedition 57 timelapse; green, yellow, and red colors described and attributed; "red shows up halfway through the video"
- [Airglow Imagery — NASA SVS 12963](https://svs.gsfc.nasa.gov/12963) — NASA visualization documentation listing red, green, and orange in ISS airglow sequences
- [Predictive modeling of 557.7 nm greenline airglow — arXiv 2504.02262](https://arxiv.org/html/2504.02262v1) — detailed altitude profile (92–105 km primary peak), production mechanism, solar cycle dependence
- [Nightglow continuum 300–1800 nm — ACP 24, 1143 (2024)](https://acp.copernicus.org/articles/24/1143/2024/) — FeO orange arc contribution, HO2 near-IR discovery, FeO altitude ~87 km, continuum flux values
- [FeO "Orange Arc" Emission Detected in Optical — NASA/Leonid Project](https://leonid.arc.nasa.gov/MS045.pdf) — FeO orange arc identification, 540–680 nm quasi-continuum, meteoric iron origin
- [Tackling the FeO orange band puzzle — MNRAS 500, 4296 (2021)](https://academic.oup.com/mnras/article/500/4/4296/5974545) — FeO spectral characterization and altitude comparison to Na and OH
- [Variations of 557.7 nm during Solar Cycle 23 — Geomagnetism and Aeronomy](https://link.springer.com/article/10.1134/S0016793219050050) — solar cycle effect on green line: ~20% amplitude, positive correlation with F10.7
- [Solar cycle effects on oxygen green line — Earth, Planets and Space](https://link.springer.com/article/10.5047/eps.2011.04.006) — confirms ~20% solar cycle amplitude on 557.7 nm
- [Sodium does not follow solar cycle — Diurnal/annual/solar cycle variations of OH and Na — ScienceDirect](https://www.sciencedirect.com/science/article/abs/pii/0032063373901475) — explicit statement that sodium intensity does not follow the solar cycle
- [Enhancement of equatorial OI(1D) at midnight — Springer](https://link.springer.com/article/10.1186/s40623-022-01596-4) — equatorial midnight enhancement, latitude-dependent double-peaked arcs, seasonal occurrence statistics
- [Night airglow phenomenology — Space Science Reviews](https://link.springer.com/article/10.1007/BF00241526) — diurnal variation pattern: intensity decreases from twilight to midnight minimum in mid/high latitude sectors
- [Post sunset behavior OI 6300 — NASA NTRS](https://ntrs.nasa.gov/citations/19770003789) — post-sunset 630 nm enhancement, asymmetric with respect to geomagnetic equator
- [Airglow variability in context of global mesospheric circulation — ResearchGate](https://www.researchgate.net/publication/222664903_Airglow_variability_in_the_context_of_the_global_mesospheric_circulation) — seasonal patterns including OH winter enhancement
- [Seasonal variation of twilight sodium layer — ScienceDirect](https://www.sciencedirect.com/science/article/abs/pii/0021916971900602) — sodium altitude shifts 5 km between summer and winter solstice
- [MANGO network geomagnetic storm impact on 630 nm — ADS](https://ui.adsabs.harvard.edu/abs/2017AGUFMSA51C2409K/abstract) — SAR arcs, LSTIDs, anomalous airglow brightening during storms
- [OH Meinel band nightglow profiles from OSIRIS — JGR Atmospheres (2014)](https://agupubs.onlinelibrary.wiley.com/doi/full/10.1002/2014JD021617) — OH altitude profile 83–87 km, 7 km FWHM
- [Airglow from the IAFE Buenos Aires](https://www.iafe.uba.ar/aeronomia/airglow.html) — seasonal patterns: high O2 in April, high OH in southern mid-winter; variations exceed an order of magnitude
- [Photographing Airglow — Lonely Speck](https://www.lonelyspeck.com/airglow/) — camera sensitivity: green dominant in photographs, eye perceives faint blue; white balance discussion
- [Upper atmospheric gravity wave details revealed in nightglow — PNAS 112 (2015)](https://www.pnas.org/doi/10.1073/pnas.1508084112) — gravity wave modulation of airglow brightness; ripple structures visible in satellite imagery
- [Airglow Imaging Observations — Observation of equatorial plasma bubbles by ISS-IMAP — PEPS](https://progearthplanetsci.springeropen.com/articles/10.1186/s40645-018-0227-0) — ISS VISI instrument, EPBs as dark voids in 630 nm airglow
