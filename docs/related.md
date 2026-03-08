# Live Earth Wallpaper - Existing Software Research

Research conducted on 2026-03-07 to evaluate existing solutions before building a custom application.

## Summary

There are quite a few existing solutions in this space. They fall into two broad categories:
1. **Satellite imagery apps** - fetch real photos from geostationary weather satellites (Himawari-8, GOES-16/17, Meteosat) and set them as wallpaper. These show the *actual* Earth but only from fixed satellite positions.
2. **Rendered globe apps** - render a synthetic view of Earth using texture maps (Blue Marble, etc.) with computed day/night lighting, city lights, and optionally overlaid cloud data. These allow arbitrary camera positions but are not "real" photos.

The original DesktopEarth was in category 2. Most modern open-source projects are in category 1.

---

## Commercial / Proprietary Software

### EarthView by DeskSoft
- **Website:** https://www.desksoft.com/EarthView.htm
- **Platforms:** Windows
- **Price:** ~$25-$35 (paid, with trial)
- **Type:** Rendered globe
- **Features:**
  - Map and globe views with day/night shadows
  - City lights, atmospheric effects, clouds
  - Seasonal vegetation, snow cover, ocean ice changes
  - High resolution output (beyond 4K)
  - Highly customizable view parameters
- **Europe view:** Yes - rendered globe with arbitrary camera position, can center directly on Europe/Germany.
- **Assessment:** The closest thing to DesktopEarth that's still maintained and commercially supported. Windows-only. Proprietary, no source code.

### EarthDesk by Xeric Design
- **Website:** https://www.xericdesign.com/earthdesk.php
- **Platforms:** Windows, macOS, Apple TV
- **Price:** $24.99 (one-time) + optional $14.99/year subscription for premium cloud data, earthquakes, ISS tracking
- **Type:** Rendered globe
- **Features:**
  - 12 map projections (Mercator, Azimuthal, Globe, etc.)
  - Near real-time cloud coverage from satellite weather data
  - Sun, moon, and city lighting
  - Multi-monitor support (different map per screen or span)
  - 4K resolution support
- **Europe view:** Yes - rendered globe with 12 projections and arbitrary camera position, can center directly on Europe/Germany.
- **Assessment:** Very feature-rich and actively maintained. Cross-platform (Windows + Mac). The closest commercial equivalent to DesktopEarth. The subscription model for cloud data is a downside.

### Downlink
- **Website:** https://downlinkapp.com/
- **Platforms:** macOS only
- **Price:** Free
- **Type:** Satellite imagery
- **Features:**
  - Satellite imagery from GOES-16, GOES-17, Himawari-8
  - Updates every 20 minutes or hourly
  - Custom views (zoom, hemisphere selection)
  - Shuffle mode, different sources per display
  - Menu bar app
- **Europe view:** No. Only uses GOES-16/17 (Americas) and Himawari-8 (Asia/Pacific). The developer noted they haven't found a suitable open data source for Europe with sufficient quality. No Meteosat support.
- **Assessment:** Excellent Mac app, but macOS only. Uses real satellite photos rather than rendered views. No Linux or Windows support. No European coverage at all.

---

## Open Source Software

### SpaceEye
- **Repository:** https://github.com/KYDronePilot/SpaceEye
- **Website:** https://spaceeye.app/
- **Platforms:** Windows, macOS
- **License:** Open source
- **Type:** Satellite imagery
- **Features:**
  - 12 views from 5 geostationary satellites (Himawari-8, GOES-16, GOES-17, Meteosat-8, Meteosat-11)
  - Up to 8K resolution
  - Updates every 10 minutes to 1 hour depending on the view
  - Available on Microsoft Store and Mac App Store
- **Europe view:** Poor. Includes Meteosat-8 and Meteosat-11, but these are geostationary over the equator (~0°E). Europe appears small and distorted at the top edge of the disk. Good view of Africa, not great for Germany/Central Europe.
- **Assessment:** Well-polished open-source option. No Linux support. Shows real satellite imagery (fixed camera positions, not arbitrary).

### Satpaper
- **Repository:** https://github.com/Colonial-Dev/satpaper
- **Platforms:** Windows, Linux
- **Language:** Rust
- **Type:** Satellite imagery
- **Features:**
  - Satellites: GOES-East, GOES-West, Himawari, Meteosat-9, Meteosat-10
  - Arbitrary wallpaper resolution including vertical
  - Configurable Earth disk size
  - Custom background image overlay
  - One-shot mode for scripting integration
  - Lightweight CLI tool
- **Europe view:** Poor. Same issue as SpaceEye - Meteosat satellites show Europe at the top edge of the disk, small and distorted. GOES and Himawari don't cover Europe at all.
- **Assessment:** Modern, well-designed CLI tool written in Rust. Good for power users. No GUI. Satellite imagery only (fixed viewpoints). No macOS builds (but could probably compile).

### Live-Earth-Wallpapers
- **Repository:** https://github.com/lennart-rth/Live-Earth-Wallpapers
- **Platforms:** Windows, Linux, macOS
- **Language:** Python
- **Type:** Satellite imagery
- **Features:**
  - All known geostationary satellites
  - Sentinel high-res images
  - NASA Solar Dynamics Observatory images
  - NASA Astronomy Picture of the Day
  - GUI and CLI modes
- **Europe view:** Poor. Same geostationary satellite limitation. Best option would be Meteosat, but Europe is at the edge. Sentinel images could provide high-res regional views of Europe but these are strips/tiles, not a full globe view.
- **Assessment:** Most comprehensive satellite coverage. Cross-platform. Python-based (requires Python or bundled executable). Actively maintained.

### SeenFromSpace
- **Repository:** https://github.com/mendhak/SeenFromSpace
- **Platforms:** Cross-platform (Python)
- **Language:** Python
- **Type:** Rendered map (2D projections)
- **Features:**
  - Day/night phase visualization
  - Night lights
  - Cloud coverage overlay
  - Satellite tracking
  - Multiple map projections
- **Europe view:** Likely yes. Supports configurable latitude, longitude, and projection parameters. Should be possible to center on Europe. However, it uses 2D map projections, not a 3D globe, so it won't look like "Earth from space".
- **Assessment:** Closest open-source equivalent to the "rendered" approach (category 2). Uses 2D map projections rather than a 3D globe. Python script, not a polished app.

### EarthLiveSharp
- **Repository:** https://github.com/bitdust/EarthLiveSharp
- **Platforms:** Windows
- **Language:** C#
- **Type:** Satellite imagery
- **Features:**
  - Himawari-8 satellite imagery
  - System tray application
  - Periodic automatic updates
- **Europe view:** No. Himawari-8 covers Asia/Pacific only.
  Europe is not visible at all.
- **Assessment:** Simple and focused. Windows-only, Himawari-8 only. Written in C# (.NET Framework 4.5), which could be a useful reference for Windows desktop integration.

### XPlanet
- **Website:** https://xplanet.sourceforge.net/
- **Platforms:** Linux, macOS, Windows (historically)
- **License:** GPL
- **Type:** Rendered globe
- **Features:**
  - Renders Earth (and other planets) from arbitrary viewpoints
  - Real-time cloud overlay from satellite data
  - Day/night with city lights
  - Configurable projections
  - Marker and arc overlays
- **Europe view:** Yes - renders from arbitrary viewpoints, can center directly on Europe/Germany.
- **Assessment:** The classic open-source solution. Very powerful but old and not user-friendly. Unclear maintenance status - the core project is largely dormant, but ecosystem tools (XPlanetFX, KDE plugins) keep it alive. Cloud image hosting by Xeric Design (same company as EarthDesk) still active.

### weather-wallpaper
- **Repository:** https://github.com/alexcohennyc/weather-wallpaper
- **Platforms:** macOS only
- **Language:** Swift/Mapbox
- **Type:** 3D rendered globe (Mapbox-based)
- **Features:**
  - 3D globe with Mapbox Standard style
  - Real-time sun position with twilight/night overlays
  - Live weather radar (RainViewer)
  - Real-time aircraft positions (OpenSky Network)
  - Pollen and air quality data
  - Auto-rotation, multiple zoom levels
  - Menu bar controls
- **Europe view:** Yes - 3D globe with search/geocode, can fly to any location including Germany. Multiple zoom levels available.
- **Assessment:** Very modern and feature-rich, but macOS only. Uses Mapbox for rendering (requires API key). Not a "real Earth from space" look, more of a stylized globe.

---

## Feature Comparison Matrix

| Feature                    | EarthView | EarthDesk | SpaceEye | Satpaper | Live-Earth-WP | SeenFromSpace | XPlanet | weather-wp |
|----------------------------|-----------|-----------|----------|----------|----------------|---------------|---------|------------|
| **Windows**                | Yes       | Yes       | Yes      | Yes      | Yes            | Yes*          | Yes*    | No         |
| **macOS**                  | No        | Yes       | Yes      | No*      | Yes            | Yes*          | Yes     | Yes        |
| **Linux**                  | No        | No        | No       | Yes      | Yes            | Yes*          | Yes     | No         |
| **Open Source**            | No        | No        | Yes      | Yes      | Yes            | Yes           | Yes     | Yes        |
| **Free**                   | No        | No        | Yes      | Yes      | Yes            | Yes           | Yes     | Yes        |
| **Arbitrary camera angle** | Yes       | Yes       | No       | No       | No             | Projections   | Yes     | Yes        |
| **3D globe view**          | Yes       | Yes       | No       | No       | No             | No            | Yes     | Yes        |
| **Day/night cycle**        | Yes       | Yes       | Natural  | Natural  | Natural        | Yes           | Yes     | Yes        |
| **City lights**            | Yes       | Yes       | No       | No       | No             | Yes           | Yes     | No         |
| **Cloud overlay**          | Yes       | Yes       | Natural  | Natural  | Natural        | Yes           | Yes     | Radar      |
| **Seasons/vegetation**     | Yes       | Limited   | Natural  | Natural  | Natural        | No            | Limited | No         |
| **Resolution**             | 4K+       | 4K        | 8K       | Custom   | Varies         | Varies        | Custom  | Custom     |
| **GUI**                    | Yes       | Yes       | Yes      | No       | Yes            | No            | No      | Yes        |
| **Update frequency**       | Continuous| Continuous| 10-60min | 10-60min | 10-60min       | Configurable  | Configurable | Continuous |
| **Europe-centered view**   | Yes       | Yes       | Poor     | Poor     | Poor           | Likely        | Yes     | Yes        |

\* = Requires Python or manual compilation; not a native packaged app.

"Natural" means the feature comes inherently from satellite imagery (clouds and day/night are in the photo).

"Poor" for Europe view means Meteosat covers the region but Europe appears small and distorted at the top edge of the satellite disk. Downlink is excluded from this table as it has no Meteosat support at all. EarthLiveSharp (Himawari-8 only) has no Europe coverage.

---

## Gap Analysis - What's Missing?

Looking at the original DesktopEarth feature set (rendered 3D globe, arbitrary viewpoint, day/night, city lights, seasons, clouds, cross-platform), **no single existing solution covers everything**.

The Europe viewpoint requirement is a key differentiator. All geostationary satellites orbit above the equator, so satellite imagery apps can never provide a nice centered view of Europe/Germany - the region will always appear small and distorted at the edge of the disk. Only rendered globe apps can provide an arbitrary camera angle that centers on Europe.

1. **EarthDesk** comes closest but costs $25 + optional subscription, is proprietary, and doesn't support Linux. Does support Europe-centered views.
2. **EarthView** is Windows-only and proprietary. Does support Europe-centered views.
3. **XPlanet** has the right feature set and supports arbitrary viewpoints, but is old, hard to use, and maintenance is unclear.
4. **SeenFromSpace** has the right approach (rendered with overlays) and can likely center on Europe, but only does 2D projections.
5. **Satellite imagery apps** (SpaceEye, Satpaper, Live-Earth-Wallpapers) look great but cannot provide a good Europe-centered view due to geostationary orbit limitations.
6. **weather-wallpaper** supports Europe views but is macOS only and uses a stylized Mapbox look rather than realistic imagery.

### A custom solution would fill the gap of:
- Free and open source
- Cross-platform (Windows priority, then Linux/Mac)
- 3D rendered globe with arbitrary camera position
- Day/night cycle with city lights
- Seasonal textures (NASA Blue Marble)
- Real-time cloud overlay from satellite data
- Modern, user-friendly GUI

---

## Recommendation

Before building from scratch, consider:

1. **Try EarthDesk first** ($25) - it covers nearly all requirements and is actively maintained. If it meets your needs, the time saved vs. building from scratch is enormous.
2. **Try SpaceEye or Satpaper** if you're okay with satellite imagery from fixed positions rather than a rendered arbitrary viewpoint.
3. **If building custom**, look at XPlanet's source and SeenFromSpace's Python code for reference implementations. EarthLiveSharp (C#) is a good reference for Windows wallpaper-setting integration.

---

## Name Ideas

| Name | Notes |
|---|---|
| **Overview** | References the "overview effect" — the cognitive shift astronauts report when seeing Earth from space. Evocative, one word, easy to remember. Risk: very generic, hard to search for. |
| **Earthrise** | The iconic 1968 Apollo 8 photograph. Instantly recognizable association. Risk: there's an existing unrelated Earthrise npm package, and the name implies a horizon view rather than a full globe. |
| **Pale Blue Dot** | Carl Sagan's famous phrase. Strong emotional resonance. Risk: long, and the Voyager photo it references shows Earth as a tiny speck — opposite of what we render. |
| **TerraView** | Latin "terra" (earth) + view. Clean, professional, clearly descriptive. Risk: generic-sounding, a few GIS tools use similar names. |
| **Orbital** | Short, punchy, implies the vantage point. Works well as a command name (`orbital`). Risk: could be confused with game engines or space sims. |
| **BluMarble** | Nod to NASA's Blue Marble imagery (our primary texture source). Distinctive spelling. Risk: too close to "Blue Marble" which is NASA's dataset name, could cause confusion. |
| **Gaia** | Greek goddess personifying Earth. Short, elegant. Risk: conflicts with ESA's Gaia space observatory and the Gaia hypothesis; heavily used name in software. |
| **Tellus** | Roman goddess of Earth, less overused than Gaia. Clean, distinctive. Risk: obscure, some people won't know the reference. |
| **Zenith** | The point in the sky directly above an observer — fitting for a "looking down" perspective. Risk: widely used brand name (electronics, insurance, etc.). |
| **Solstice** | Evokes seasons and astronomical cycles, both core features. Risk: doesn't directly suggest "Earth" or "wallpaper". |
| **EarthCanvas** | Descriptive: Earth rendered onto your desktop canvas. Risk: a bit literal/bland. |
| **Mundus** | Latin for "world." Short, unique, unlikely to conflict. Risk: obscure, could be misread. |
| **Aphelion** | The point in Earth's orbit farthest from the Sun. Astronomical, distinctive. Risk: obscure, and the meaning doesn't relate to what the app does. |

### Domain-friendly names (gTLD combinations)

Relevant gTLDs that are open for public registration: `.earth`, `.space`, `.blue`, `.world`, `.live`, `.zone`, `.vision`, `.observer`, `.watch`, `.solar`, `.global`. Note: `.sky` exists but is closed (owned by Sky Ltd).

**gTLD pricing** (Cloudflare sells at wholesale cost with no markup, good baseline; Porkbun is also competitive):

| gTLD | Cloudflare (at cost) | Porkbun | Notes |
|---|---|---|---|
| .observer | $9.18/yr | — | Cheapest relevant TLD |
| .earth | not available | $15.96/yr | Cloudflare doesn't carry it |
| .blue | $19.20/yr | $20.08/yr | Reasonable |
| .space | $25.20/yr | $26.26/yr | OK |
| .live | $25.20/yr | — | OK |
| .zone | $30.20/yr | — | Getting pricey |
| .world | $32.20/yr | — | Getting pricey |
| .watch | $34.20/yr | — | Pricey |
| .vision | $35.20/yr | — | Pricey |
| .solar | $50.20/yr | — | Expensive |
| .global | $75.20/yr | — | Very expensive |

Registration = renewal at Cloudflare (no bait-and-switch). Prices as of March 2026.

**Name + domain candidates** (availability checked 2026-03-08):

| Domain | Name | ~Cost/yr | Available? | Notes |
|---|---|---|---|---|
| `sunlit.earth` | Sunlit | ~$16 | **Yes** | Short, poetic, directly evokes the sunlit side of Earth. |
| `earthshine.space` | Earthshine | ~$25 | **Yes** | Real astronomy term: the faint glow on the moon's dark side from light reflected by Earth. Beautiful, distinctive. |
| `earthshine.blue` | Earthshine | ~$19 | **Yes** | Same name, cheaper TLD, "blue" reinforces the Earth color. |
| `earthward.space` | Earthward | ~$25 | **Yes** | The direction of the gaze — looking earthward from orbit. |
| `earthward.blue` | Earthward | ~$19 | **Yes** | Same name, cheaper TLD. |
| `overview.blue` | Overview | ~$19 | **Yes** | The "overview effect" + blue planet. |
| `daylit.earth` | Daylit | ~$16 | **Yes** | Similar to sunlit, evokes the illuminated Earth. |
| `nightfall.earth` | Nightfall | ~$16 | **Yes** | Evocative, but emphasizes night only. |
| `nightside.earth` | Nightside | ~$16 | **Yes** | The dark side with city lights. Astronomical term. |
| `perigee.earth` | Perigee | ~$16 | **Yes** | Closest point in orbit to Earth. Short, distinctive. |
| `nadir.earth` | Nadir | ~$16 | **Yes** | The point directly below the observer — fitting for "looking down at Earth." |
| `terminator.earth` | Terminator | ~$16 | **Yes** | The day/night boundary line on a planet. Real astronomy term, but strong pop-culture baggage. |
| `lookback.earth` | Lookback | ~$16 | **Yes** | Looking back at Earth from space. |
| `duskline.earth` | Duskline | ~$16 | **Yes** | The terminator line at dusk. Poetic. |
| `bluehour.earth` | Blue Hour | ~$16 | **Yes** | The twilight period when the sky turns deep blue. Atmospheric. |
| `earthturn.earth` | Earthturn | ~$16 | **Yes** | The rotation of the Earth — but redundant TLD. |
| `earthturn.space` | Earthturn | ~$25 | **Yes** | Same name, no TLD redundancy. |
| `halfearth.space` | Half Earth | ~$25 | **Yes** | Half in light, half in shadow. |
| `globeview.earth` | Globeview | ~$16 | **Yes** | Simple and clear. |
| `earthpaper.space` | Earthpaper | ~$25 | **Yes** | Earth + wallpaper portmanteau. |
| `dailyearth.space` | Daily Earth | ~$25 | **Yes** | Updated daily. |
| `skyward.earth` | Skyward | ~$16 | **Yes** | Looking skyward — though we're looking the other way. |
| `earthlit.space` | Earthlit | ~$25 | **Yes** | "Earth, lit" by the sun. |
| `earthseen.space` | Earth Seen | ~$25 | **Yes** | "Earth, seen from space." |
| | | | | |
| `pale.blue` | Pale Blue | ~$19 | Taken | Sagan reference. Registered since 2016. |
| `overview.earth` | Overview | ~$16 | Taken | Registered since 2022. |
| `earthrise.space` | Earthrise | ~$25 | Taken | Registered Jan 2026, parked on Afternic (squatter). |
| `marble.earth` | Marble | ~$16 | Taken | Registered since 2015. |
| `orbital.earth` | Orbital | ~$16 | Taken | Registered Jun 2025. |
| `from.space` | From Space | ~$25 | Taken | Registered since 2019. |
| `blue.earth` | Blue Earth | ~$16 | Taken | Registered since 2015 (Blue Dot Capital LLC). |
| `terra.vision` | Terra | ~$35 | Taken | Registered since 2016. |
| `earthshine.earth` | Earthshine | ~$16 | Taken | Registered since 2021. |
| `penumbra.earth` | Penumbra | ~$16 | Taken | Registered Oct 2025. |
| `halflight.earth` | Halflight | ~$16 | Taken | Registered Feb 2026. |
| `earthrise.blue` | Earthrise | ~$19 | Taken | |
| `marble.blue` | Marble | ~$19 | Taken | |

### Decision

**Sunlit Earth** — domain: `sunlit.earth` (~$16/yr, available as of 2026-03-08)

Short, poetic, directly evokes the core feature (the sunlit side of Earth as seen from space). No meaningful conflicts in software — the name was used for a long-dead macOS Dashboard widget (~2006) and has no presence on crates.io, npm, or PyPI. Other uses of the name are unrelated (photography blog, essential oils brand, a Shadow of the Colossus music track, a band). No trademark in the software category. The crate name `sunlit-earth` is available.
