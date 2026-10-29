# Research: Solar Eclipses

Written 2026-10-29, from the wish to support solar eclipses in three ways: draw the Moon's shadow on the Earth where and when it falls, render the Sun well when the Moon passes in front of it as the camera sees it, and jump to the previous or next solar eclipse from the time bar. The question for the second part is whether it is worth building at all, since the camera rarely sits where the Moon covers the Sun. The third became eclipse mode, a camera that follows the shadow, in the discussion that followed (section 5.3). Section 7 records the decisions taken with the user; the plan is `2026-10-29-solar-eclipse-plan.md`.

The numbers here come from the code as of `ca634ac` and from scripts that use the Python port of Astronomy Engine (`astronomy-engine` on PyPI, the same library and models as the C build the app links) with the app's own world frame, camera and composite rebuilt in NumPy. The scripts are not committed; each section says what was computed.

## 1. What the app already has

**The inputs.** `scene::sky::SkyState` already holds the two things a shadow needs. `sun_direction` is `Astronomy_GeoVector(BODY_SUN, ABERRATION)` rotated into the world frame and normalized, and `moon_position` is `Astronomy_GeoMoon` in Earth radii in the same frame, through `world_from_eqj`, which applies precession and nutation and the Greenwich apparent sidereal time. Astronomy Engine's own eclipse search builds its shadow from exactly these two vectors (`MoonShadow` in `astronomy.c`: the aberrated geocentric Sun and the geometric geocentric Moon), so no new astronomy is needed for the geometry. What is missing is the Sun's distance: `body_direction` normalizes it away, and the Sun's apparent radius, which decides total against annular, varies by 3.4% over the year.

**The Moon over the Sun in the sky.** Half of a camera-side eclipse is drawn already. The Moon is drawn opaque after the Sun's disk, so it covers the disk by draw order, and `scene::disk_occlusion::visibility` counts it as a second occluder, so the glare, the corona needles and the halo fade with the share of the disk it hides (`plans/2026-08-23-celestial-c-moon-plan.md`, departure 1). `the_moon_covers_the_sun_at_the_2024_total_eclipse` already holds the ephemeris to the 2024-04-08 eclipse.

**What is not usable as it is.** The Moon's `moon_model` uniform carries `moon_size` (2.5 by default) and the pixel floor `MIN_BODY_DISK_RADIUS_PIXELS` in its scale, so the shader has no true Moon radius. Nothing on the globe, the clouds or the atmosphere reads the Moon at all.

The research of 2026-08-23 (`plans/2026-08-23-celestial-bodies-research.md`, section 6.5) sketched the shadow as "the Earth shader receives the Moon's world position and darkens fragments whose view of the sun is occluded" and deferred it. That sketch holds; the sections below fill it in.

## 2. The shadow

### 2.1 Size, speed and frequency

Computed from `SkyState`'s inputs at the peak of each eclipse of 2026 to 2029, in the plane through the Earth's center perpendicular to the shadow axis (a negative umbra is the antumbra of an annular eclipse; on the ground the umbra is a few tens of km wider, being nearer the Moon, and stretched where the axis meets the ground obliquely):

| Peak (UTC) | Kind | Umbra diameter | Penumbra diameter | Axis from Earth's center | Ground speed of the axis |
|---|---|---|---|---|---|
| 2026-02-17 | Annular | -144 km | 7111 km | 6214 km | 0.97 km/s |
| 2026-08-12 | Total | 107 km | 6859 km | 5725 km | 0.95 km/s |
| 2027-02-06 | Annular | -324 km | 7293 km | 1882 km | 0.58 km/s |
| 2027-08-02 | Total | 200 km | 6766 km | 905 km | 0.67 km/s |
| 2028-01-26 | Annular | -352 km | 7321 km | 2490 km | 0.48 km/s |
| 2028-07-22 | Total | 141 km | 6825 km | 3864 km | 0.60 km/s |
| 2029-01-14 | Partial | | 7175 km | 6732 km | |

So the penumbra is a disc a little more than one Earth diameter across, a soft darkening that covers a large share of the visible hemisphere, and the umbra is a spot of 100 to 250 km inside it, about one texel of the 512 px day floor and a handful of the finest tiles. Between 1900 and 2100 Astronomy Engine finds 454 solar eclipses, 2 to 5 in each calendar year, 29.3 to 178.3 days apart. Each lasts a few hours from the penumbra's first contact with the Earth to its last.

At the 5 minute wallpaper refresh the umbra moves about 150 to 300 km between frames at the peak, faster near the ends of the track where it meets the ground obliquely. That is about one umbra width, which is acceptable for a wallpaper; the preview at live time redraws on the two minute sky schedule.

These agree with what NASA publishes. Espenak's animation page for the 2002 eclipse describes the penumbra from space as "a large greyish region" about 6900 km across and the umbra as "a tiny black dot" about 87 km wide (https://eclipse.gsfc.nasa.gov/SEmono/TSE2002/TSE2002txt/T02animate.html). NASA Earth Observatory gives path widths of 110 km for 2017 and 185 km for 2024, and its 2017 page describes the ISS view of the shadow near the limb, with "the upper levels of the atmosphere faded from blue to black" over it (https://science.nasa.gov/earth/earth-observatory/so-many-ways-to-view-an-eclipse-90796, https://science.nasa.gov/earth/earth-observatory/total-solar-eclipse-darkens-north-america-152663). Neither source gives the penumbra's gradient or any color cast in numbers, so the look in section 3.3 is a decision rather than a measurement.

### 2.2 How exactly the app's inputs place it

For each central eclipse of 2026 to 2030 the shadow axis, built from the app's own aberrated Sun direction and `GeoMoon` in the world frame, was intersected with the unit sphere and compared with the peak location `Astronomy_SearchGlobalSolarEclipse` reports, converted from geodetic to geocentric latitude:

| Peak | Kind | Astronomy Engine (geodetic) | Axis on the sphere (geocentric) | Apart |
|---|---|---|---|---|
| 2026-02-17 12:11 | Annular | 64.72 S, 86.71 E | 64.58 S, 85.09 E | 78 km |
| 2026-08-12 17:45 | Total | 65.22 N, 25.25 W | 64.95 N, 25.96 W | 36 km |
| 2027-02-06 15:59 | Annular | 31.29 S, 48.47 W | 31.11 S, 48.47 W | 2 km |
| 2027-08-02 10:06 | Total | 25.49 N, 33.19 E | 25.33 N, 33.19 E | 1 km |
| 2028-01-26 15:07 | Annular | 2.97 N, 51.55 W | 2.95 N, 51.55 W | 0 km |
| 2028-07-22 02:55 | Total | 15.60 S, 126.71 E | 15.49 S, 126.71 E | 1 km |
| 2030-06-01 06:27 | Annular | 56.51 N, 80.07 E | 56.24 N, 80.08 E | 10 km |
| 2030-11-25 06:50 | Total | 43.61 S, 71.24 E | 43.38 S, 71.24 E | 4 km |

Where the axis meets the ground steeply the two agree to a few km. The two large differences are the eclipses whose peak is near the polar limb, where the axis meets the ground at a grazing angle and the difference between the app's sphere and Astronomy Engine's ellipsoid moves the intersection far along the ground. Two more error sources are of the same order or smaller: the cube faces put geodetic latitude on the sphere's direction, while the shadow falls by geocentric geometry, up to 0.19 degrees or 21 km apart at 45 degrees; and `SkyState` is computed to the whole second, during which the umbra moves under 2 km.

The library's own accuracy is stated as within one arcminute in general, with no figure for the Moon alone (README, https://github.com/cosinekitty/astronomy). Its test suite (`generate/ctest.c`) checks 1180 solar eclipses from 1701 on against Espenak's Five Millennium Catalog: the peak time to within 7.56 minutes, and the peak location of central eclipses whose axis passes within 6100 km of the Earth's center to within 0.247 degrees, about 27 km. Its local eclipse test holds contact times for specific observers to within a minute. So the library promises tens of km for the shadow's place, and the agreement above is the app reproducing the library, not the library against the sky. That is ample for a picture in which the umbra is a few pixels across at most zooms.

**Precision in f32.** The angle between the Sun and the Moon seen from a point near totality is about 0.27 degrees, whose cosine is 1 minus 1.1e-5, about 185 steps of f32 below 1. Taking `acos` of a dot product of two normalized directions therefore resolves the angle to about 1.3e-5 rad at the umbra's edge, about 5 km on the ground before the rounding of the dot itself, and to about 3.5e-4 rad (some 130 km) on the axis, where the umbra is. The fix is not to measure that angle at all (section 3.1).

### 2.3 The ellipsoid without an ellipsoidal globe

The cube bake puts the Blue Marble's latitude, which is geodetic, at the direction from the center whose elevation is that latitude (`tools/texture-pipeline`, `asin(direction.y)` in `test_cube.py`). The geodetic latitude is by definition the elevation of the ellipsoid's normal, so every point of the drawn sphere points the way the true surface normal at that place does: the lighting `n . l` is already the true local altitude of the Sun, and the terminator is already right. What the sphere gets wrong is position, which the Sun's lighting does not depend on and the Moon's shadow does. Both error sources of section 2.2 are that.

So the shadow can be computed on the true Earth without changing the globe: a fragment at the sphere's direction `n` stands for the WGS84 surface point `N * (n.x, (1 - e^2) * n.y, n.z)` with `N = a / sqrt(1 - e^2 * n.y^2)`, `a` the equatorial radius and `e^2 = f * (2 - f)`, `f = 1 / 298.257223563`, and a shell at height `h` for that point plus `h * n`. The light-left function is evaluated there. The shadow's center is the axis's intersection with the ellipsoid, read back as a geodetic latitude, which is what the camera's latitude means.

The same six eclipses as section 5.5, the shadow's center found by scaling the world's +Y by `1 / (1 - f)` so the ellipsoid becomes a sphere, and the contacts by minimizing the cone's distance over the ellipsoid (Nelder-Mead from the point nearest the axis), differences in seconds from NASA's maps on Astronomy Engine's ΔT, and the peak point against Astronomy Engine's:

| Eclipse | P1 | U1 | U4 | P4 | Peak point |
|---|---|---|---|---|---|
| 2023-10-14 annular | +1 | +1 | -3 | -2 | 0.0 km |
| 2024-04-08 total | 0 | -3 | +1 | -2 | 0.0 km |
| 2025-03-29 partial | +2 | | | -2 | |
| 2026-02-17 annular | +3 | +7 | -4 | 0 | 0.0 km |
| 2026-08-12 total | -4 | -9 | -1 | -6 | 0.0 km |
| 2027-08-02 total | -2 | -4 | -1 | -4 | 0.0 km |

Against the sphere's 72 seconds and 78 km at worst, the ellipsoid comes to within 9 seconds of NASA and to Astronomy Engine's own peak point, which also uses an ellipsoid. On screen the sphere's worst case is about 5 px at the default zoom, where the globe is about 772 px across at 1080 px, and about 75 px at the closest, where a pixel is about a km.

Drawing the globe itself as an ellipsoid was weighed and set aside with the user: the flattening is 0.335%, 2.6 px from pole to pole on the default globe, against a change to everything that measures the globe as a sphere (the mesh and the texture lookup, the painted globe in the composite and `sun_occlusion`, `disk_occlusion`'s globe, `limb_extinction`, the residency's horizon, `display::layout`'s framing, the globe drag in `mouse_math`, the shells) and every golden reference on every adapter.

## 3. Drawing the shadow

### 3.1 The light left at a point

For a point `P` on the globe or on a shell, in world space in Earth radii, the share of sunlight that reaches it is the share of the Sun's disk the Moon's disk leaves uncovered, as seen from `P`. In the frame of the shadow axis that needs only subtractions and one length, which is what keeps it exact in f32:

- On the CPU, once per frame: the axis direction `a = normalize(M - S)` from the Sun's center `S` through the Moon's center `M`, the Moon's center, the distance `D` from the Moon to the Sun, and the true radii `R_moon` (1737.4 km, the mean radius Astronomy Engine uses) and `R_sun` (695,700 km), all in Earth radii.
- In the shader: `q = P - M`, `z = dot(q, a)` the distance past the Moon along the axis, `r = length(q - z * a)` the distance from the axis. Seen from `P`, the Moon's center and the Sun's center are both `r` off the axis, at distances `z` and `z + D`, so the angle between them is `atan(r / z) - atan(r / (z + D))`, the Moon's angular radius `asin(R_moon / length(q))` and the Sun's `asin(R_sun / length(P - S))`. Every quantity is a difference of numbers of order 60, rounded at about 4e-6 Earth radii, about 25 m.
- The uncovered share is then the circle-circle overlap of those three angles, the same lens area `disk_occlusion::overlap_area` computes on the CPU for the glare. Small-angle geometry on the tangent plane is exact to well under a percent at half a degree.

Outside the penumbra the share is exactly 1. The CPU knows each frame whether the penumbra reaches the outermost shell at all (the axis's distance from the Earth's center against the penumbra's radius plus the shell's), so a flag bit can skip the whole computation in uniform control flow. Outside the few hours of each eclipse every fragment then runs what it runs today, and every golden reference and engine case stays bit for bit what it is; only frames during an eclipse pay. Inside the penumbra the cost is a handful of transcendentals per fragment on the globe and on each shell that uses it, which needs measuring on WARP before the plan sets a budget, as the Milky Way and the clouds were.

The function belongs in two places, Rust and WGSL, like `limb_transmission`: Rust for the tests and for the CPU gate, WGSL for the frame. That makes it a fourth rule for `the_shader_and_the_cpu_agree_on_the_three_shared_rules`.

### 3.2 Limb darkening and color

The Sun is not a uniform disk: its limb is darker and redder than its center. Two laws were found with a source: at 550 nm the quadratic `0.3 + 0.93 mu - 0.23 mu^2` from Allen's Astrophysical Quantities (4th ed., as quoted at https://en.wikipedia.org/wiki/Limb_darkening), and at 640.97 nm Neckel and Labs's (Solar Physics 153, 1994) fifth-order polynomial with coefficients 0.33644, 1.30590, -1.79238, 2.45040, -1.89979, 0.59943 (from HAO's `lmbdrk_sbsp.pro`). A blue law with a citable source was not found. With a Moon 1.02 times the Sun's apparent radius, a numerical integration over the disk gives:

| Moon's center from the Sun's, in solar radii | Area left | Light left, 550 nm | Light left, 641 nm |
|---|---|---|---|
| 1.60 | 0.888 | 0.902 | 0.900 |
| 1.20 | 0.703 | 0.707 | 0.706 |
| 0.80 | 0.481 | 0.460 | 0.463 |
| 0.50 | 0.298 | 0.263 | 0.268 |
| 0.30 | 0.172 | 0.137 | 0.142 |
| 0.15 | 0.077 | 0.052 | 0.056 |
| 0.10 | 0.045 | 0.028 | 0.031 |
| 0.04 | 0.009 | 0.004 | 0.005 |

So the area alone overstates the light by a quarter to a half in the deep partial phase, the inner few hundred km of the penumbra, and is within a few percent over most of it. After the display gamma (section 3.3) that is a few levels of brightness in a region that is already dark. The tint is small: the red light left is 9 to 11% more than the green in the last tenth of the partial phase, and blue would fall further below green, but without a sourced blue law that is a direction rather than a number.

Three ways to carry it, in increasing cost: the uniform disk's area alone; a correction from a small table in the uniform block, indexed by the uncovered area, which is nearly a function of that alone for the narrow range of Moon to Sun size ratios seen from the ground (0.92 to 1.08); or an integration over a few annuli of the disk in the shader, inside the gated branch. The first gets the shape, the edges of the umbra and the antumbra and the extent of the penumbra right, and the other two only adjust brightness and tint within it, so the uniform disk is the place to start and the look decides whether to go further.

### 3.3 From the light left to pixels

The globe is drawn on the stored texture values with a tone map that compresses sunlight hard: `diffuse_floor` is 0.70, so ground under a grazing Sun is drawn at 70% of noon, and the clouds' night value is "an appearance parameter rather than an irradiance" (rendering.md, Clouds on the night side). The eclipse has to be mapped through the same kind of decision. The candidates:

- **Multiply the stored day value by the light left, `F`.** Simple, but the stored values are gamma encoded, so half the light draws at half the value, which reads much darker than half the light: the penumbra would be darker than the twilight side of the terminator.
- **Multiply by `F^(1/2.2)`,** the light left re-encoded as the stored values are. Half the light draws at 0.73, a tenth at 0.35, a hundredth at 0.12. This is what a linear camera such as DSCOVR's shows, and puts the visible darkening in the inner half of the penumbra with a dark core. It is the natural default.
- **The umbra as a local night.** At `F` of zero the day term is black, but under the umbra the ground is lit about as a moonlit night is, and the app's rendition of a moonlit night is the night texture. Mixing the night texture in as `F` approaches zero, by a weight that is zero until the deep partial phase, draws the umbra as the night side is drawn, city lights included where it crosses a city. Mixing it in across the whole penumbra, by handing `F` to `blend_fragment` as if it were the Sun's altitude, would draw city lights at half daylight, which no camera would see. The user chose darkening alone (section 7), since a bright city under the few minutes of an umbra is an unusual sight.

Every term that is sunlight needs the same factor: the day surface after `blend_fragment`'s shading (after its `min(night, day)` clamp, which would otherwise hold the eclipsed day up), the specular glint, and the Fresnel shift's sky color, which already scales with the day blend. `fs_cloud` needs it on the day brightness and on the share of the deck drawn at the day opacity, since real eclipse imagery shows the shadow most clearly on cloud tops. `fs_rayleigh` needs it on the day blue and on the sunrise lobe. The nightglow shells do not.

Which surface modes draw it is a decision. Blend mode is the obvious one. Day mode lights the globe evenly and ignores the Sun on purpose, and the grid ignores it in `fs_main` too, though its shells follow the Sun; drawing the shadow on the grid would make it testable on the golden suite's grid frames and visible to anyone checking where the Moon is, the reason the grid keeps a real terminator. The user chose blend mode only (section 7).

**Tiles.** In blend mode the wanted set asks for night tiles only where the Sun's `n . l` is below `terminator_width` (rendering.md, The wanted set), so an umbra on the day side drawn as night would be drawn from the 512 px night floor, about 20 km a texel, under an umbra 100 to 250 km wide. If the umbra showed the night texture, `residency::drawn` would have to want night tiles inside it; with darkening alone nothing changes there.

### 3.4 Settings

One parameter covers it: a strength for the Moon's shadow, 0 to 1 with 1 the mapping of section 3.3 and 0 the switch, in the Celestial group beside the Moon's, which is the fourteen edits of a shader parameter. Whether it needs more (an exponent, a separate umbra weight) is for the look. Neither `moon_size` nor the pixel floor should reach the shadow: it is drawn from the true geometry, since a Moon drawn 2.5 times larger casting a 2.5 times larger shadow would put a penumbra over most of the hemisphere at every new moon near a node.

### 3.5 Tests

Following the conventions in testing.md, behavior rather than constants:

- The shared-rule probe compares the WGSL light-left function against the Rust one along a line through the umbra, the penumbra and outside it, at a total and an annular geometry.
- An engine case at the peak of 2024-04-08 finds the darkest pixel of the globe, reprojects it to latitude and longitude, and holds it within a few tens of km of Astronomy Engine's peak (25.29 N, 104.14 W), and an annular case holds the antumbra's center lit above zero and below the penumbra around it.
- At a time with no eclipse the frame is bit for bit the frame without the feature, which is what the gate promises.
- One golden over the Earth fixture with the cloud fixture, at a time when the penumbra and the umbra are both in view.

### 3.6 What photographs of the shadow show

The first look at the shadow as built (`F^(1/2.2)`, a uniform disk) read as a dark spot with a hard rim. Photographs of the shadow from orbit were gathered on 2026-10-29, each page fetched and each image confirmed to load. The user picked these as the useful ones:

| Image | Eclipse | Vantage | What it is good for |
|---|---|---|---|
| Mir 27 crew / CNES, [page](https://science.nasa.gov/image-article/apod-1999-august-30-looking-back-on-an-eclipsed-earth), [image](https://assets.science.nasa.gov/content/dam/science/cds/apod/apod/1999/august/eclipse99_mir_big.jpg) | 1999-08-11 | Low orbit, oblique, over cloud | The umbra's edge and the falloff, the best view |
| ISS Expedition 71, [page](https://science.nasa.gov/image-detail/iss071e003390-lrg), [image](https://science.nasa.gov/wp-content/uploads/2023/06/iss071e003390-lrg.jpg) | 2024-04-08 | Low orbit, toward the horizon | High quality, though the angle is not ideal; cropped and enhanced for contrast by NASA |
| DSCOVR EPIC, [page](https://science.nasa.gov/earth/earth-observatory/an-epic-eclipse-87675/), [frames](https://assets.science.nasa.gov/content/dam/science/esd/eo/images/imagerecords/87000/87675/eclipse_epc_2016068.zip) | 2016-03-09 | L1, the full sunlit disk | Scale: the visible shadow against the hemisphere; 13 frames 20 minutes apart |
| Himawari-8 (NICT), [still](https://i0.wp.com/cosmiclog.com/wp-content/uploads/2016/03/160309-himawari.jpg?ssl=1), [CIMSS page](http://cimss.ssec.wisc.edu/goes/blog/archives/20899) | 2016-03-09 | Geostationary, full disk | Scale, the same eclipse as EPIC from closer; the CIMSS band 1 frame is named for 01:50 UTC |
| GOES-16/18, CIMSS, [post](https://cimss.ssec.wisc.edu/satellite-blog/archives/58552) | 2024-04-08 | Geostationary | Scale only: minimum composites of the 0.86 µm band with a contrast stretch, not natural color |

What they agree on, read by eye from the Mir, ISS 2006, EPIC 2024 and Himawari images: no picture shows a hard edge at the umbra. The darkening is deepest over the umbra, the visible shadow reaches well beyond it, several times its width, and the falloff is gradual but not regular, and the outline is not a circle. Much of the irregularity is the shadow multiplying a scene of bright cloud and dark ground, which the app's rendering gets on its own, since the shade multiplies the textures. The brown cast of the EPIC and MODIS images is not taken as real: those instruments build color from narrow bands chosen for science, not for human color vision, and the Mir and ISS photographs, from ordinary cameras, show a neutral grey.

Two physical effects soften the edge and were left out of the first build: limb darkening, which takes light from the thin crescent that is all that is left near the umbra (section 3.2), and light scattered into the umbra by the air around it, which keeps the umbra's floor above black. Neither is measured from space in numbers found, so both are judged against the photographs.

**Limb darkening in the shader.** Section 3.2 offered a table by the uncovered area. Measured over Moon to Sun ratios of 0.92 to 1.08 against a fine integration of the 550 nm law, the light left is not quite a function of the area alone: the best cubic in the area is off by up to 0.019, and by up to 44% of the light where little is left, which is the region near the umbra that the look turns on. Splitting the disk into rings of uniform brightness does better, each ring's covered share from the circle overlap at its two radii and its weight the integral of the law over it. Rings of equal area are poor at the limb; rings of equal width in `mu` with exact weights give:

| Rings | Largest error | Largest relative error where more than 0.3% is left |
|---|---|---|
| 3 | 0.0073 | 14% |
| 4 | 0.0053 | 6% |
| 6 | 0.0023 | 5% |
| 8 | 0.0011 | 1.5% |

Four rings, at `mu` 1, 0.75, 0.5, 0.25 and 0, are radii 0, 0.6614, 0.8660, 0.9682 and 1 of the Sun's, with weights 0.5107, 0.3088, 0.1458 and 0.0348 of the disk's light (the law's integral `0.3 mu^2 + 0.62 mu^3 - 0.115 mu^4` between the ring's edges, over its value 0.805 at `mu` 1). That is four circle overlaps where the uniform disk takes one.

## 4. The Moon in front of the Sun, seen from the camera

### 4.1 When it happens

The Moon covers part of the Sun as the camera sees it when the camera is inside the Moon's penumbra. The sky lens is conformal, so the angular test is the screen test, and `disk_occlusion` already measures it in screen circles. Two things make it rare in this app.

First, the camera looks at the Earth. Yaw and pitch move the look-at point by at most one Earth radius, so the Sun is in frame only when the camera is on the night side looking toward it past the globe. Second, the composite paints the globe larger than the sky lens images it: at the defaults (8 Earth radii, `camera_fov` 20, `sky_fov` 86) the painted globe reaches 0.715 of the half short edge, which the sky lens reaches at 31.4 degrees from the view axis, where the Earth's true angular radius is 7.2 degrees. A Sun nearer the view axis than that is behind the painted globe. So the camera has to sit on the night side about 4 Earth radii off the line from the Earth to the Sun, inside a penumbra about 1.1 Earth diameters wide. That is mostly a new moon at which the shadow misses the Earth: the camera's eclipses are a different set of events from the Earth's.

A simulation over 2026 to 2035, a minute at a time within 1.5 days of every new moon, with the app's world frame, eye, lenses and composite rebuilt at a 16:9 output with no pan, yaw or pitch, counted the minutes in which the Moon's disk overlaps a Sun in frame and outside the painted globe. Those ten years hold 22 solar eclipses on the Earth.

| Camera | Moon at true size | Moon at the default 2.5 times |
|---|---|---|
| Earth-fixed, the default 30 N 0 E at 8 radii | 151 min at 6 new moons, never covered | 595 min at 15 new moons, 21 min covered |
| Earth-fixed, 30 N 0 E at 3.2 radii | none | 2 min at 1 new moon |
| Earth-fixed, 30 N 0 E at 20 radii | 250 min at 15 new moons | 578 min at 20 new moons, 91 min covered |
| Earth-fixed, 30 N 0 E at 80 radii | none | none |
| Sun-fixed, 30 N at 02:00, 8 radii | 825 min at 8 new moons | 2629 min at 15, 228 covered |
| Sun-fixed, 30 N at 20:00, 8 radii | 659 min at 7 new moons | 2468 min at 14, 276 covered |
| Sun-fixed, 30 N at 06:00 or 18:00, 8 radii | none | none |

At 80 radii the camera is beyond the Moon's orbit, where full moons can put it in the shadow instead; a run within three days of every full moon found it for a Sun-fixed camera at midnight on the equator twice in ten years, and never for the Earth-fixed default.

So at the defaults the camera sees the Moon touch the Sun at about one or two new moons a year, for an hour or so each, and with the Moon at its true size it never sees it covered: the camera would have to be in the umbra, which is at most a few hundred km wide and often does not exist. Everything "total" the camera sees is the 2.5 times Moon covering a Sun the true Moon would leave partly bare; for the same reason the camera can never show an annular eclipse at the default size.

### 4.2 What it would look like on screen

At the defaults on a 1080 pixel tall picture the Sun's disk is 3.2 pixels in radius and the Moon at 2.5 times 7.8. The glare, which is what makes the Sun read as the Sun, already fades with the share of the disk left (by its square root, so a sliver still glares, which is most of the diamond ring's look). What a real total eclipse has and the app lacks:

- **The corona,** a soft glow out to a few solar radii, of the order of a millionth of the disk's surface brightness just above the limb and a thousandth of that within a solar diameter (https://en.wikipedia.org/wiki/Solar_corona). Espenak's exposure guide steps down by a factor of two per exposure value, six steps from 0.1 to 1 solar radius and four more to 8 radii (whether the table counts from the limb or the center was not checked) (https://eclipse.gsfc.nasa.gov/SEpubs/20060329/tables/table_23.html), which is a tone mapped falloff and the more useful guide for a picture. On screen it would be a halo some 10 pixels across around the Moon, visible only with the Sun nearly covered, which at the true Moon size the camera never sees.
- **The diamond ring and Baily's beads.** The glare is centered on the Sun's center, not on the light that is left. Moving its center to the centroid of the uncovered crescent is cheap on the CPU and gives a crescent's glare its right place. The beads are sub-pixel at any field of view the slider offers.

Neither is expensive, but both serve an event that happens about once a year at the defaults, mostly unannounced (it is not an eclipse anyone could look up), and at a few pixels. A narrower `sky_fov` magnifies it, and a user who wants to see it can be helped to find it (section 5). The recommendation is to leave this part out of the first plan, keep what `disk_occlusion` already does, and come back to the glare's centroid and a corona only if screenshots ask for them.

## 5. Finding and following an eclipse

### 5.1 The search

Astronomy Engine has the search: `Astronomy_SearchGlobalSolarEclipse(start)` finds the first solar eclipse after `start` and `Astronomy_NextGlobalSolarEclipse(peak)` the one after a previous peak. Both search forward only; there is no backward search in the C API. The result carries the kind (partial, annular or total), the peak, which is the instant the shadow axis passes nearest the Earth's center (greatest eclipse), the axis's distance from the center, and for central eclipses the latitude and longitude of the axis on the ellipsoid at the peak and the share of the Sun's disk covered there, which is 1 for a total eclipse. The header says the position and the share are undefined for a partial eclipse. The C bindings already expose all three functions and the struct. The search walks the new moons, prunes those whose ecliptic latitude rules an eclipse out, and refines the rest.

The previous eclipse follows from the forward search: start 200 days before the instant and step forward until a peak passes it; the last one before it is the answer. 200 days is enough because the longest gap between consecutive solar eclipses from 1900 to 2100 is 178.3 days. The Python port took 5.3 ms a search over those two centuries; the C library was not timed, and a few searches per click is cheap either way. The time setting is held to the years 1900 to 2100 (`ui_callbacks.rs`, `instant_of_sliders`), so previous and next stop at the first and the last eclipse inside that range.

Two limits of the library's classification. It has no hybrid kind: the hybrid eclipses of 2013-11-03 and 2023-04-20 come back as total, the kind at the peak. And a partial eclipse has no latitude and longitude, since the axis misses the Earth; the point on the sphere nearest the axis is the natural stand-in (section 5.4).

### 5.2 Marks on the date track

The date track already carries the four season diamonds, 9 px glyphs with a label each (`SeasonMark`, `time_bar.rs`). The 2 to 5 solar eclipses of the displayed year go on it the same way, as 9 px glyphs whose light part is in `OverlayStyle.sun`, so they do not read as seasons, and whose shape says the kind: a total eclipse is a dark disk with a thin bright rim, an annular one a thick bright ring around a smaller dark center, a partial one a bright disk with a dark bite out of one side. Hovering a mark replaces the track's date label with the event, such as "Annular solar eclipse · Tue 17 Feb 12:11". A click on a mark turns eclipse mode on at that eclipse (section 5.3); it is a target of its own over the track, so a press on a mark jumps rather than starting a drag, and the track's 3 day snap to seasons does not reach it.

Two eclipses are never closer than 29.3 days, so eclipse marks never meet each other, but they do meet the season marks: of the 454 eclipses from 1900 to 2100, 16 fall within a day of an equinox or a solstice (2034-03-20, a total eclipse 0.12 days from the March equinox, is the nearest in the coming decades) and 54 within six days, which on the 540 px track of an 800 px bar is about the width of one glyph. Where the two overlap the eclipse mark is drawn over the diamond and the hover label names both.

### 5.3 Eclipse mode

Decided with the user on 2026-10-29, in place of a plain jump: a button turns on eclipse mode, which writes the peak of an eclipse to the time setting and from then on keeps the camera over the Moon's shadow as the time of day moves, whether by the day track, by exact entry or by playing the clock. Previous and next move the mode from one eclipse to the next. Leaving the mode leaves the camera where it is.

Which eclipse the button takes: the one whose penumbra is on the Earth at the instant on screen, and otherwise the next one. Scrubbing onto an eclipse and then pressing the button therefore stays on that eclipse.

What leaves the mode, agreed with the user:

- the mode button again;
- Now, and turning the settings window's Date / Time override off, which both return to live time;
- a write of the time setting that changes the displayed day: a pick on the date track, the date picker, the override's Day slider. A write that keeps the day, the day track, exact time entry and the override's Hour slider, keeps the mode;
- playback of the date, whose days a second would swing the camera around the globe;
- a hand on the camera's position: the globe drag, the orbit stick and arrows, the orbit keys, a preset, the Sun-fixed switch. Zoom, look, frame, roll and north up keep the mode, since the mode decides only where over the Earth the camera hangs.

On the way out the camera's position as last resolved is written into the position settings, as the latitude and the longitude, or the time of day when the Sun-fixed mode is on, so the frame does not jump and a drag that ended the mode carries on from what was shown. That is the conversion the Sun-fixed switch already does (`ui_callbacks::convert_camera_frame`).

The mode is saved with the other camera settings, so the wallpaper draws what the preview draws and a restart comes back to it. The eclipse it is on need not be saved: none of the actions that keep the mode can move the time setting off the eclipse's day, and eclipses are at least 29.3 days apart, so the eclipse whose peak is nearest the time setting is the one. That changes the earlier proposal of saving the peak, for a flag alone.

The app already has a mode built the same way. `SceneParams::camera_at(sky)` resolves the Sun-fixed camera for each frame from the sky that frame is drawn with, in `write_uniforms`, in the tile outputs and in the frame key (`plans/2026-10-16-sun-fixed-camera-plan.md`, decision 8). Eclipse mode is a third case of that function which sets the latitude and the longitude from the Sun and the Moon of the frame's sky. So the camera follows during an excursion, in every export and in the wallpaper without any setting being written per frame.

### 5.4 Where the camera looks

The camera hangs over the point where the shadow axis meets the Earth, and over the point nearest the axis when it misses. With `a` the axis direction from the Sun through the Moon, `c` the point of the axis nearest the Earth's center and `|c|` its distance in Earth radii, the near intersection is `c - sqrt(1 - |c|^2) * a`, which at `|c| = 1` is `c` itself, so one expression covers both cases and is continuous. At the peak of a partial eclipse that point is on the limb, where the deepest part of the penumbra falls: for 2025-03-29 it lands at 61.10 N, 77.16 W, against 61 N, 77 W in NASA's catalog for greatest eclipse.

The square root makes the point race when the axis enters or leaves the Earth, where its derivative is infinite. Softening it to `sqrt(max(0, 1 - |c|^2) + eps^2)` bounds the speed, at the price of hanging the camera off the axis's ground point by up to some angle while the axis is on the Earth, the most at the moment it touches the limb. Over the six eclipses of section 5.5, between the penumbra's first and last contact, sampled 1500 times, on the sphere:

| `eps` | Fastest the point moves | Farthest off the axis's ground point |
|---|---|---|
| 0 | 8 to 16 degrees a minute, bounded only by the sampling | 0 |
| 0.2 | 0.8 to 2.4 degrees a minute | 8.7 to 10.4 degrees |
| 0.35 | 0.6 to 1.4 | 16.5 to 18.4 |
| 0.5 | 0.5 to 0.9 | 23.7 to 25.7 |

The median speed is 0.3 to 0.5 degrees a minute in every case, and the point travels 105 to 164 degrees over an eclipse. At a Clock speed of 1 min/s and `eps` 0.2 the camera turns at most 2.4 degrees a second, and its offset from the shadow is at most 10 degrees of arc, about 1100 km, which at the default distance keeps the umbra well inside the globe's disk. At the peaks of the central eclipses of 2026 to 2030 the offset is 18 to 463 km, the most for the annular eclipse of 2026-02-17, whose axis passes 0.974 Earth radii from the center; for a partial eclipse, whose axis misses, the softened point sits `atan(0.2 / |c|)`, about 11 degrees, sunward of the limb point, which shows the penumbra on the limb less obliquely. So 0.2 is the proposal, for the look to confirm, and whatever reports where the shadow is uses the unsoftened point.

The camera's latitude is held to 89.9 degrees either side by `OrbitalCamera::new`, which a shadow over a pole meets without harm.

### 5.5 Contact times

Astronomy Engine reports the peak but not when the shadow touches and leaves the Earth: P1 and P4, the first and last contact of the penumbra, and U1 and U4, those of the umbra or the antumbra. They follow from the cone geometry: with the penumbral cone's apex between the Moon and the Sun, its half angle `f1 = asin((R_sun + R_moon) / D)` and the Earth's center at `z` along the axis from the apex and `rho` off it, the cone touches the sphere when `rho * cos(f1) - z * sin(f1) = 1`; the umbral cone, apex beyond the Moon and half angle `f2 = asin((R_sun - R_moon) / D)`, touches it when `rho * cos(f2) - |z| * sin(f2) = 1`, the absolute value taking the antumbra's nappe. A root search on those, a two minute scan within six hours of the peak and bisection, was run on the app's sphere of one equatorial Earth radius with Astronomy Engine's radii (1737.4 km and 695,700 km) and compared with Espenak's maps (https://eclipse.gsfc.nasa.gov/SEplot/SEplot2001/, one GIF per eclipse, read 2026-10-29). The maps give UT under a ΔT predicted when they were drawn, 80.7 to 84.8 s, against Astronomy Engine's 73.7 to 76.0 s, so NASA's times were moved onto Astronomy Engine's ΔT (TD less Astronomy Engine's ΔT) before comparing. Differences, the app's sphere less NASA, in seconds:

| Eclipse | P1 | U1 | U4 | P4 |
|---|---|---|---|---|
| 2023-10-14 annular | -10 | -14 | -3 | -1 |
| 2024-04-08 total | -2 | -4 | +12 | +6 |
| 2025-03-29 partial | -1 | | | +23 |
| 2026-02-17 annular, Antarctic | -20 | -72 | +41 | +1 |
| 2026-08-12 total, Arctic | -22 | -53 | +15 | -6 |
| 2027-08-02 total | -6 | -10 | 0 | -2 |

So the sphere's contacts are within half a minute of NASA's mostly and within 72 seconds at worst, the worst where the shadow meets the Earth near a pole. There the sphere stands 21 km above the ellipsoid, which would make a first contact earlier and a last one later, the sign seen in the two polar eclipses. Times shown to the minute are right or one minute off. On the ellipsoid the same contacts are within 9 seconds (section 2.3), which is what the plan uses.

Over 1900 to 2100 the penumbra is on the Earth for 21 minutes (1935-01-05) to 6.2 hours (2096-11-15), and the umbra or antumbra, at the 300 eclipses where it touches at all, for 10 minutes (2086-12-06) to 4.0 hours (2096-11-15). On a time track of the day that is a band 1.5% to 26% of the track's width.

### 5.6 What else can be shown truthfully

The duration of totality or annularity at greatest eclipse: `Astronomy_SearchLocalSolarEclipse` for an observer at the peak's latitude and longitude gives the start and end of the central phase there. Against NASA's figures for duration at greatest eclipse (the Besselian element pages, https://eclipse.gsfc.nasa.gov/SEbeselm/SEbeselm2001/):

| Eclipse | Astronomy Engine at the peak point | NASA | Apart |
|---|---|---|---|
| 2023-10-14 annular | 5m11.4s | 5m17.2s | -5.8 s |
| 2024-04-08 total | 4m31.7s | 4m28.1s | +3.6 s |
| 2026-02-17 annular | 2m16.1s | 2m19.6s | -3.5 s |
| 2026-08-12 total | 2m21.3s | 2m18.2s | +3.1 s |
| 2027-08-02 total | 6m26.9s | 6m22.6s | +4.3 s |

Totals come out longer and annulars shorter, which a larger Moon than NASA's umbral `k2 = 0.2722810` would do; the radius the local search uses was not checked. Within six seconds, so a duration shown to the second carries an error of a few seconds.

For a partial eclipse the largest share of the Sun covered is at the point nearest the axis at the peak: the local search there gives 0.930 of the disk's area for 2025-03-29. NASA quotes the magnitude, the share of the diameter, 0.9361 on its map, which is a different quantity, so the panel says area ("93% of the Sun covered") and does not call it magnitude.

The coverage at the shadow's center at the instant on screen, the unsoftened point of section 5.4, is one minus the light-left function of section 3.1 there, which needs nothing new. The point the camera follows is not the place to measure it: at the peak of 2026-08-12 it is 243 km from the axis's ground point, outside an umbra 107 km wide.

Not available: place names, since the app carries no geographic names; the Saros series and the path width, which the library does not provide and which mean little in a wallpaper app.

### 5.7 The controls in the time bar

The open bar is 124 px tall and at most 800 px wide. The transport (skip, the two rulers of speeds, play) is centered over the tracks, and at full pitch it is 400 px wide (`transport-fixed` 124 px and twelve dots of 11 px a side); the tracks' column is the bar's width less 260 px, 540 px at the full width, which leaves 70 px either side. The speed readout takes the left; on the right there is nothing. Below 800 px the dots pack closer, down to 7 px, and at the narrowest bar, 564 px, the transport fills the whole column. Two previous and next buttons beside a mode button need about 75 px, which fits only at the full width, so the user chose one button there and a panel above the bar for the rest. The one 28 px button and a 6 px gap raise the narrowest bar to 598 px.

The panel opens above the bar while the mode is on and the bar is open, and collapses with it. It holds, top to bottom: previous and next on either side of the kind's glyph, the kind and the peak's date in the displayed zone; the phases with their times, partial begins, total or annular begins, peak, total or annular ends, partial ends, the current one highlighted and each a click that sets the time to it and keeps the mode; the duration at greatest eclipse, or for a partial eclipse the largest share covered and its latitude; and a line for the instant on screen, before ("Shadow reaches Earth in 1 h 12 min"), during (the coverage and the latitude of the shadow's center) or after ("Shadow left Earth 20 min ago").

While the mode is on the day track shows the eclipse instead of the Sun's altitude over the camera, which would mean nothing over a moving camera (the bar already skips that shading for the Sun-fixed camera): a muted band in the sun color for P1 to P4, a stronger one for U1 to U4, and a line at the peak. A band cut by midnight in the displayed zone ends in a small arrow at the track's edge.

The bar's hover hints sit 8 px above the bar (`main.slint`, the hint's `y`), and the panel would cover those of the controls beneath it, so with the panel up they go above the panel. The camera panel sits at the preview's top right (`x: parent.width - self.width - 12px`, 92 px wide); a panel over the bar's right end reaches it only in a preview narrower than about 1000 px and short enough for the two to meet, which the plan has to handle.

Keys: Page Up and Page Down are bound nowhere in the preview's `track-key` or its `KeyBinding`s and do not depend on the keyboard layout; `[` and `]` would need AltGr on a German keyboard. The letters W, A, S, D, Q, E, H, F, J, K, L, T, G, R and N are taken; E in particular is roll.

### 5.8 Lunar eclipses later

`Astronomy_SearchLunarEclipse` has the same shape, and the Earth's shadow on the Moon is the same light-left function with the Earth as the occluder, in `fs_moon`. They are out of scope here.

## 6. Recommendation

1. **Build the shadow** (section 3): the light-left function in the axis frame, gated per frame on the CPU so frames outside an eclipse are untouched, applied in blend mode to the globe, the clouds and the Rayleigh shell with the `F^(1/2.2)` mapping, darkening only, one strength setting, the uniform disk first. This is the visible feature, computed on the ellipsoid it lands where Astronomy Engine puts it (section 2.3) with the inputs the app already has, and it comes for free to every user at live time during a real eclipse.
2. **Build eclipse mode** (section 5): the forward and backward search, marks on the date track, the mode button and its panel, the camera that follows the shadow through `camera_at`, the contact times by the cone geometry of section 5.5, and the eclipse bands on the day track. It is what makes the shadow findable outside the few hours a year it is live, and it keeps it in view while it moves.
3. **Leave the camera-side rendering as it is** (section 4). The occlusion and the glare's fade already exist; at the defaults the camera sees the true Moon touch the Sun about once a year and never cover it, at a few pixels. The glare's centroid and a corona are small follow-ups if wanted later.

## 7. Decisions

Taken with the user on 2026-10-29:

1. The shadow is drawn in blend mode only.
2. The umbra only darkens: no night texture and no city lights in it, so the wanted set of tiles does not change.
3. Choosing an eclipse writes the time setting, as the tracks do, not an excursion.
4. The camera is moved so the eclipse is visible, and in eclipse mode it follows the shadow.
5. The jump goes to the peak.
6. The camera-side improvements of section 4.2 are left out.
7. Previous and next are not a plain jump but part of eclipse mode (section 5.3), with the exits listed there.
8. One mode button at the right end of the transport row, and a panel above the bar for previous, next and the eclipse's details (section 5.7), shown whenever the bar is open and the mode is on.
9. While the mode is on, the day track shows the eclipse's phases.
10. The eclipse's geometry, the shadow drawn, its center, the camera's point and the contact times, is computed on the WGS84 ellipsoid; the drawn globe stays a sphere (section 2.3).

Taken after the first look, the same day (section 3.6):

11. The clouds under the shadow keep their day opacity and are darkened as the ground is.
12. The Sun's disk carries limb darkening. The shadow stays neutral grey, with no color cast.
13. Three settings let the look be tuned by eye against the photographs: the brightness inside the umbra, how far the shadow reaches beyond it, and how fast it brightens.
14. No randomness is added to the shadow's outline for now; the three settings are tried first.
