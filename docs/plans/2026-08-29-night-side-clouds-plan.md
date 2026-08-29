# Plan: Clouds on the Night Side

Written 2026-08-29, from the observation that clouds on the night hemisphere render nearly
black, where photographs of the night side show them plainly. The reference the user named
is Reid Wiseman's "Hello, World" from Artemis II, and that photograph turns out to answer
the question more sharply than expected: it is a photograph of the night side alone, and
what makes its clouds bright is measurable and is not radiometry.

The defect itself is one hardcoded constant and two places that shade the cloud shell as
though it were the ground. What takes a document rather than a one-line commit is choosing
the constant, because no physical calculation produces it and the reason why is worth
writing down once. On top of the fix sits one addition the user asked for after reading the
first draft: a night-side cloud should pick up the city light thrown onto it from below,
under a control of its own.

## What it measures to today

`fs_cloud` in `shaders/sphere.wgsl`:

```wgsl
let brightness = mix(0.05, 1.0, smoothstep(-uniforms.terminator_width, uniforms.terminator_width, n_dot_l));
return vec4<f32>(brightness, brightness, brightness, cloud_density * uniforms.cloud_opacity);
```

The 0.05 traces to `2026-03-18-cloud-layer-rendering-plan.md` decision D3, which called it
"night brightness of 0.05 (faint ambient)", named it a v1 choice, and said a follow-up
could add dayside shading. Nothing has touched it since.

Every format in the pass is `Rgba8Unorm` and never `*Srgb`, so the whole composite runs in
display-referred space: the sRGB-encoded texture bytes are sampled as if linear, blended as
if linear, and written out to be read back as sRGB. There is no exposure and no tone curve
anywhere. So 0.05 is not an irradiance, it is 5 percent of display white, code value 13 of
255. Against a dayside cloud at 1.0 that is 7.82 stops of linear light.

The number that matters more is what it composites over. The night map is
`BlackMarble_2016.jxl`, and `night_gamma` and `night_saturation` both default to 1.0, so its
bytes reach the blend unchanged. Measured on the asset as it sits in `textures/`, decoded to
2048x1024 and sampled as 8 to 24 texel patches:

| what | R, G, B | luminance |
|---|---|---|
| mid-Pacific ocean | 0.020, 0.020, 0.063 | 0.023 |
| central Sahara, unlit land | 0.138, 0.127, 0.237 | 0.137 |
| Antarctica | 0.165, 0.200, 0.329 | 0.202 |
| western Europe, 24x24 over the cluster | 0.214, 0.198, 0.207 | 0.202 |
| Nile delta, 16x16 | 0.390, 0.353, 0.362 | 0.361 |
| a city core | saturated at 1.0 | 1.0 |

Black Marble is not a black image with lights on it. It carries a dark blue terrain base,
and unlit land sits at 0.137, brighter than Antarctica is dark and nearly six times the
ocean. So with `cloud_opacity` at its default 0.85 and a fully dense cloud, straight alpha
gives `0.05 * 0.85 + dst * 0.15`:

| under the cloud | before | after |
|---|---|---|
| ocean, 0.023 | 0.023 | 0.046 |
| unlit land, 0.137 | 0.137 | 0.063 |
| a city cluster, 0.361 | 0.361 | 0.097 |
| a city core, 1.0 | 1.0 | 0.193 |

That is the defect stated in the app's own numbers. A night-side cloud is darker than the
land it covers, and it takes a city down by a factor of five. Clouds read as dark blotches
and as a veil over the lights, which is the opposite of both the physics and the pictures:
a cloud has an albedo of 0.5 to 0.9 against land's 0.1 to 0.3, and city light reaching a
cloud base is scattered back up, which is what makes cloud decks over cities visible in
VIIRS Day/Night Band imagery at all.

## Research

Recorded 2026-08-29. Web sources were read by a subagent and are cited below; the
photographic measurements, the exposure arithmetic and the texture measurements above were
made in this session with ImageMagick and are marked measured; arithmetic on sourced
figures is marked derived.

### The physical gap is about 18.6 stops

Full-moon illuminance at the surface is put at 0.1 to 0.3 lux against roughly 32,000 to
130,000 lux for direct sunlight. The citable form of it is the ratio itself: Plait's widely
reproduced calculation gives sun to full moon as about 400,000 to 1, other sources 450,000,
which is 18.61 to 18.78 stops (derived, `log2`). Cloud albedo, 0.5 to 0.9, multiplies
whatever arrives, so the ratio carries from the illumination to the clouds unchanged: a
moonlit cloud and a sunlit cloud return the same fraction of very different amounts of
light. Airglow and starlight together come to 0.0002 to 0.002 lux, another 100 to 500 times
down, 26 to 27 stops below sunlight, which is to say that nothing but the Moon matters as a
global term.

City lights are the exception in kind rather than in degree. The Day/Night Band resolves
individual streetlamps and shows diffuse city glow through cloud decks, but the mechanism is
spatially concentrated over populated land, so a uniform ambient term cannot represent it.
It is the one night-side source with structure worth drawing.

### Cloud tops keep the Sun about three degrees longer

A point at height `h` above a sphere of radius `R` loses direct sunlight when the Sun is
`acos(R / (R + h))` below the local horizontal. For Earth that is 3.2 degrees at 10 km, 3.5
at 12, 3.9 at 15, 4.3 at 18 (subagent's calculation, cross-checked here). An independent
source on sunset color states that a 10 km cloud overhead keeps direct sun until the Sun is
about 3 degrees down, which lands within 0.2 degrees of the geometry.

Our shell is `CLOUD_SPHERE_RADIUS = 1.0015`, which is 9.6 km, and the same condition in the
form the shader wants is `n_dot_l = -sqrt(1 - 1/r^2) = -0.0547`, 3.14 degrees (derived).
`fs_rayleigh` already computes exactly that expression for the tangent condition of its own
shell, as `earth_limb_ndotv`, so the form is already in the house.

Beyond the geometry there is twilight. Civil twilight spans 0 to 6 degrees of solar
depression, over which illuminance falls from roughly 585 to 2 lux, so a cloud top that has
lost the direct beam is still lit by a sky that is hundreds of times brighter than full
moonlight for the first few degrees, and the source above notes an effective screening
height of about 3 km for red light against 11 km for blue, so the last light a high cloud
sees is reddened. Together those say the cloud layer's terminator should be shifted
nightward by about 3 degrees and be softer than the ground's, and that its rim should warm.

### "Hello, World" is a photograph of the night side, and its clouds measure 0.53

Wikipedia and the Commons file record what the image is: Wiseman shot it through an Orion
window with a Nikon D5, 14 to 24 mm f/2.8 lens at f/4, 1/4 second, ISO 51,200, from about
9,889 km, and it "shows the night-side of the Earth. Although the Earth is still illuminated
by the full moon, it is quite dark compared to a Sun-lit Earth. Thus, Wiseman photographed
it with a longer exposure." City lights are visible across it, the bright spot near the
center is a window reflection, and the frame is rotated about 146 degrees from north up. It
was processed in Lightroom Classic.

Measured on the 1920 pixel Commons rendition, resampled to 900 wide, as patch means in
display sRGB:

| patch | R, G, B |
|---|---|
| bright cloud mass, mid-left | 0.510, 0.536, 0.556 |
| bright cloud, lower right | 0.523, 0.547, 0.562 |
| cloud-free ocean | 0.317, 0.381, 0.454 |
| space, off the disk | 0.031, 0.026, 0.029 |

So the night-side clouds in the reference photograph sit at about 0.53 of display white,
roughly two stops under it, and the moonlit ocean at about 0.38. The clouds also run blue,
about (0.95, 1.00, 1.03) normalized on green, but that image was white balanced by hand and
the cast cannot be separated from the balance, so it is not evidence about the light.

### What makes them bright is the exposure, and the amount is measurable

Both photographs state their own settings, so the gap between them can be computed rather
than guessed. Scene exposure value at ISO 100 is `log2(N^2 / t) - log2(S / 100)`:

| photograph | aperture | shutter | ISO | scene EV100 |
|---|---|---|---|---|
| "Hello, World", Artemis II, 2026 | f/4 | 1/4 s | 51,200 | -3.00 |
| "The Blue Marble", Apollo 17, 1972 | f/11 | 1/250 s | 64 | +15.53 |

The gap is 18.53 stops (derived; the Apollo film speed is taken as Ektachrome's 64, which is
the one input here that is an assumption rather than a citation, and a speed of 80 or 100
would move the result by 0.3 to 0.6 stops). The sun to full moon ratio gives 18.61.

Two independent routes, agreeing to under a tenth of a stop. That is the finding this whole
document rests on. The night side really is about 18.6 stops darker than the day side, and
the reason the clouds in "Hello, World" read at 0.53 rather than at black is that the
photographer spent all 18.5 of those stops on them. Blue Marble spent none, which is why
Apollo's night side is not merely dark but absent.

It follows that no single exposure shows a bright day side and visible night-side clouds at
once. The gap is 18.5 stops; a modern sensor's raw range is about 14, and the eye's
instantaneous range is put at 10 to 14, reaching perhaps 24 only after minutes of
adaptation. Every image that shows both is a composite, a tone map, or an eye that moved.

For this renderer that settles the question of what kind of quantity the night-side cloud
brightness is. The app draws one frame, at one exposure, with both hemispheres in it, in
display-referred space with no tone curve. It is therefore in the position of a tone-mapped
composite, and the night-side cloud value is an appearance parameter: a decision about how
much of a real 18.6-stop gap to compress into the 8 bits on screen. The present 0.05 is not
that decision. It is neither physical, being about 11 stops too bright for moonlight, nor
photographic, being 4 stops darker than the reference photograph. It is an arbitrary middle,
which is exactly what D3 said it was.

### What other renderers do

**Wrap and half-Lambert.** `(n_dot_l + w) / (1 + w)`, and Valve's Source engine form
`pow(n_dot_l * 0.5 + 0.5, 2)` exactly, introduced as a cheap stand-in for
subsurface-scattering-like falloff rather than as physics. A sensible base for a translucent
scattering medium and not for an opaque diffuse one.

**Ambient floor terms.** Several volumetric cloud implementations in the SIGGRAPH course
lineage sample the sky at a few fixed directions for a top and a bottom color and
interpolate by altitude, giving clouds a non-zero floor where all direct light has been
absorbed. Architecturally that is the same term proposed below, driven by sky samples
instead of a constant.

**Multiple scattering as octaves.** Wrenninge's art-directable multiple scattering, carried
into games by Schneider and Vos for Horizon Zero Dawn and generalized by Hillaire for
Frostbite: evaluate single scattering several times with attenuated density, phase
eccentricity and intensity, and sum. This is what keeps the shadow side of a thick cloud off
black in a volumetric renderer. The subagent could not trace the circulating attenuation
coefficients to a primary source and said so, so no numbers from it are used here. Related
and separate: the powder term `1 - exp(-k d)` from the same talk, and forward scattering
through the bulk with Henyey-Greenstein or Cornette-Shanks, single-lobe `g` typically 0.76
to 0.9, which is the silver lining proper.

**Whole-planet treatments.** SpaceEngine solves the dynamic range with simulated eye
adaptation rather than a floor, which is to say it simulates the photographic answer
directly; its dev blog describes the ISS looking like an explosion as the virtual eye
readapts crossing into sunlight. Elite Dangerous uses a flat ambient plus a cloud brightness
constant, and its forums carry the cautionary case: over-tuning that ambient makes the dark
side so clear that it stops reading as night. KSP's EVE falls back to a flat glow on the
unlit hemisphere when its Scatterer integration is off. No published, named technique for
clouds picking up city light at planetary scale was found; the nearest concept is froxel
volumetrics for local fog, which is not this.

## What changes, and what does not

Unchanged: the shell as a second sphere with straight alpha over the composited globe, the
draw order, `cloud_floor` and `cloud_gamma` and what they mean, the fetcher, the cache, the
variant selection, and the dayside appearance at the default settings.

| Today | After |
|---|---|
| Night-side clouds are 0.05 of display white, darker than the unlit land under them. | A `cloud_night` control, 0.25 by default, which puts them above the terrain base and below the city cores. |
| The night floor is a shader constant with no way to change it. | A slider in the Clouds group, persisted, in the digest. |
| The shell is shaded with the unit-sphere normal, so cloud tops share the ground's terminator. | Shaded against the shell's own tangent condition, 3.14 degrees nightward, by the expression `fs_rayleigh` already uses. |
| The cloud terminator is the globe's `terminator_width`, and in the three single-texture modes that uniform carries the sentinel -1.0. | The clouds have their own width, wider than the globe's, and never see the sentinel. |
| A cloud over a city replaces it with flat gray. | The cloud picks up a blurred sample of the night map, so a deck over a city glows and the lights spread rather than being covered, under a control whose zero is the switch. |
| No test in the suite renders a cloud at all. | A fixture cloud source in the golden engine and two cases across the terminator, plus engine cases for the ordering claims. |

## The sentinel bug

Separate from the appearance question and worth its own line, because it is a defect rather
than a choice. `render_pass::write_uniforms` sets `terminator_width` to `-1.0` whenever the
mode is not Blend, as the sentinel that tells `fs_sphere` to ignore the Sun. `fs_cloud`
reads the same uniform raw, so in Grid, Day and Night modes its call becomes
`smoothstep(1.0, -1.0, n_dot_l)`. The edges are reversed. Computed by the standard formula
that inverts the ramp, making clouds bright at local midnight and dark at noon; the
specification calls a reversed-edge `smoothstep` indeterminate, so it is wrong either way
and what it does may differ between adapters. The default mode is Blend, index 3, so the
default path is unaffected and this has been invisible.

Giving the clouds their own width fixes it as a side effect, which is the reason to do that
rather than to reuse the globe's with a `max`. Verifying it wants a render in each of the
three modes with a cloud texture present, which today nothing in the suite can produce; see
the tests section.

## Decisions

**D1. The night-side floor is an appearance parameter, and it says so.** No calculation
produces the number, because the physical answer renders as black and the photographic
answer is whatever exposure was chosen. So it is a control with a documented default rather
than a derived constant, and the doc comment on it states that it is a compression of an
18.6-stop gap rather than an irradiance. This is the same refusal the Sun plan's decision 7
made when it declined to build an HDR pipeline, arrived at from the other side.

**D2. The default is 0.25, chosen against the night map rather than against a photograph.**
The competing anchors are the reference photograph at 0.53, which is the night side given
the whole exposure, and the subagent's own inferred 0.5 to 2 percent of dayside linear
brightness, which is 0.065 to 0.14 in display terms and is a scene-referred floor meant to
be tone mapped afterwards. Neither transfers directly. What does transfer is the composition
this shader actually draws into: unlit land at 0.137, Antarctica and a typical city cluster
at 0.20, the Nile delta at 0.36, cores at 1.0. A floor of 0.25 puts a night cloud above
every unlit surface, near a moderate city cluster, and well below the cores, which is the
ordering the physics asks for. It is 4.3 stops under a dayside cloud, inside the 3 to 5 stop
band a tone-mapped photograph of a lit scene shows. The slider runs 0.0 to 0.6, so 0.05
remains reachable for anyone who wants what the app does now and 0.53 for anyone who wants
the Artemis look; Elite Dangerous's forums are the warning about the top of that range.

**D3. Shade the shell against its own geometry.** One constant derived from
`CLOUD_SPHERE_RADIUS` at 0.0547, subtracted in the comparison, so the cloud terminator sits
3.14 degrees nightward of the ground's. It is free, it is right, and the expression is
already in the file.

**D4. Clouds get their own terminator width, defaulting wider than the globe's.** The
globe's default is 0.1, which is 5.74 degrees. The cloud layer wants the geometric 3.14
degrees of shift plus a few degrees of twilight softening, so a default of 0.18, which is
10.4 degrees, is the defensible starting point rather than a measured value. This also
removes the sentinel from `fs_cloud`'s reach. Whether it needs a slider is the one open
question in this plan: it is a second terminator control in a UI that already has one, and
the argument for holding it as a constant is that nobody wants to tune two terminators. The
recommendation is a constant in `params.rs` beside `CLOUD_SPHERE_RADIUS`, revisited if the
default proves wrong.

**D5. No tint.** The measured blue cast in the reference photograph cannot be separated from
its white balance, and moonlight is spectrally close to sunlight. A neutral gray floor,
which is what the shader already writes, is the honest default. The warm rim that the
red-versus-blue screening height argues for is real but belongs with the horizon amendment's
transmission model rather than as a second color invented here.

**D6. Nothing about the dayside changes.** Wrap lighting, a phase function, powder, and a
dayside falloff toward the terminator are all defensible and all darken or complicate what
currently reads well. D3 of the original cloud plan deferred dayside shading and this plan
does not pick it up. It is a roadmap item, not a line in this change.

**D7. City-light coupling is in scope, and it is a control.** It is what turns "less dark"
into "like the photographs", and it is the only part of this with a structural cost. The
user asked for it on 2026-08-29 and asked that its weight be adjustable and that zero turn
it off, which is what `cloud_city_gain` below is: the same idiom `sun_glow`,
`star_intensity` and `moon_brightness` already use, where the bottom of the slider is the
switch and no separate checkbox exists to disagree with it. Zero has to mean the effect is
gone rather than merely small, so it gates the sample rather than scaling its result to
nothing, and a run at zero must reach exactly the pixels part one produces. Details and the
cost are in part two.

The two parts below are an implementation order, not a scope boundary. Part one cannot
break anything that works today; part two is the half that touches a bind group, so it goes
second and lands with its own tests. Both are in this change.

## Part one: the floor, the shell, the width

Small and self-contained. Following the checklist in `CLAUDE.md`:

1. `params.rs`: `CLOUD_TERMINATOR_WIDTH: f32 = 0.18` beside `CLOUD_SPHERE_RADIUS`, and a
   `cloud_night: f32` field on `SceneParams` with its `ParamsDigest` entry and its row in
   the table-driven digest test.
2. `config.rs`: `cloud_night: f32` defaulting to 0.25, in `AppConfig`, and the two
   round-trip fixtures that enumerate every field.
3. `uniforms.rs`: `cloud_night` in place of `_pad3`, which sits at offset 196 in the
   cloud and atmosphere block, so the 480-byte assertion and every later offset are
   untouched. `cloud_terminator` in place of `_pad4`.
4. `render_pass.rs`: both assigned in `write_uniforms`. `cloud_terminator` comes from the
   new constant unconditionally, never from `params.terminator_width`, which is what fixes
   the sentinel.
5. `sphere.wgsl`: the uniform block gains both names with their offsets, and `fs_cloud`
   becomes

   ```wgsl
   let shell_shift = sqrt(1.0 - 1.0 / (uniforms.cloud_sphere_radius * uniforms.cloud_sphere_radius));
   let w = uniforms.cloud_terminator;
   let sunlit = smoothstep(-shell_shift - w, -shell_shift + w, n_dot_l);
   let brightness = mix(uniforms.cloud_night, 1.0, sunlit);
   ```

   with a comment naming the tangent condition and pointing at `fs_rayleigh`'s use of the
   same expression, and one naming what `cloud_night` is per D1.
6. `main.slint`: a `cloud-night` property and a fourth `SettingRow` in the Clouds group,
   "Night side", 0.0 to 0.6, percentage value text, with a hint saying what it stands for.
   Label wording follows the group's existing short forms.
7. `ui_callbacks.rs`: the two bridge functions follow from the struct.

Nothing in this part touches a bind group, a pipeline, or the texture path, so it cannot
affect load order, memory, or the resolution switch.

## Part two: clouds lit by the cities under them

The mechanism, and it is cheap: binding 3 of the shared bind group layout is a second
texture, and the cloud bind group currently fills it with the 1x1 dummy under a comment
saying `fs_cloud` does not use it. `Renderer` already holds `night_texture_view` for the
composite group. So `maybe_create_cloud_bind_group` can bind the night map there, and
`fs_cloud` can add a blurred sample of it to the night term:

```wgsl
var upwelling = vec3<f32>(0.0);
if uniforms.cloud_city_gain > 0.0 {
    let dims = textureDimensions(night_texture, 0);
    let level = max(log2(f32(dims.x) / 1024.0), 0.0);
    upwelling = uniforms.cloud_city_gain
        * textureSampleLevel(night_texture, sphere_sampler, in.uv, level).rgb;
}
let night = uniforms.cloud_night + upwelling;
```

The branch is what makes zero mean off rather than small, per D7: at zero nothing is
sampled, so the pixels are part one's exactly and a dummy or missing night texture cannot
contribute a rounding error. It is uniform across the draw, so it costs nothing on any
adapter.

Deriving the mip level from the texture's own width rather than fixing it is what makes the
blur a fixed angle rather than a function of the resolution setting: at 1024 texels across,
one texel is 39 km at the equator, which is the scale a cloud deck spreads light over. A
fixed level would blur three times as far at 8192 as at 2048. The `max` against zero is for
a source narrower than 1024, which the two Earth maps are not but a fixture may be.

The parameter, following the same checklist part one does:

1. `params.rs`: `cloud_city_gain: f32` on `SceneParams`, its `ParamsDigest` entry, and its
   row in the table-driven digest walk.
2. `config.rs`: `cloud_city_gain: f32` defaulting to 0.7, and the round-trip fixtures.
3. `uniforms.rs`: in place of `_pad5` at offset 204, which is the last of the three spare
   slots in the cloud and atmosphere block that this change consumes. The 480-byte
   assertion and every offset after it are untouched.
4. `render_pass.rs`: assigned in `write_uniforms`.
5. `sphere.wgsl`: the uniform and the block above, with a comment saying that this is an
   extrapolation rather than a published technique.
6. `main.slint`: a fifth `SettingRow` in the Clouds group, "City glow", 0.0 to 2.0, with a
   hint saying that it is the light cities throw onto the cloud bases and that zero turns it
   off.

Three costs, all real and none large:

- **The purge hazard.** `purge_file_backed_slots` calls `Texture::destroy` on the night
  texture and clears `night_texture_view`, but it deliberately does not clear the cloud
  bind group, because the cloud slot is not file-backed. With this change that group would
  reference a destroyed texture and the next draw would be a validation error. So the purge
  has to rebuild the cloud group with the dummy in place, and `process_decoded_textures`
  has to rebuild it again when the night slot lands. That is two call sites and it is the
  only part of this plan that can break something that currently works. A resolution switch
  is the path that exercises it, and `tests/engine.rs` already has cases that switch with a
  cloud source present.
- **The mode question.** In Grid and Day modes there is no night texture to sample, and in
  a checkout without the Git LFS objects there is none in any mode. The dummy answers zero,
  which degrades to part one's behavior exactly, so the fallback is the correct one and
  needs no flag.
- **It is not a documented technique.** The subagent found no published treatment of
  city-lit clouds at planetary scale, so this is an extrapolation, and the code should say
  so where it sits rather than implying a citation.

What it buys is the one thing the flat floor cannot: a dense deck over Tokyo currently goes
from 1.0 to 0.19, and with the floor alone to 0.36, and with the coupling to something
brighter than the surrounding cloud, which is what the Day/Night Band actually shows. The
default of 0.7 is a starting point rather than a measurement: over the Nile delta patch at
0.361 it adds about 0.25 to a night term of 0.25, which doubles the cloud there and leaves
the ocean untouched, and it is the number to revisit against a real frame before the change
is called done.

## Tests

The suite renders no cloud pixel anywhere, and that has to be fixed before either part can
be called verified. The cloud slot is fed by the fetcher rather than by
`config.texture_paths`, so `golden.rs` has never had a cloud texture and the draw is skipped
in all twelve cases; `engine.rs` uses `cloud_opacity` only as a "something changed" knob for
the dirty check. So:

1. **A cloud fixture.** `tests/support` gains a generated cloud map next to
   `write_moon_fixture` and `write_panorama_bands_fixture`, and the golden engine gains a
   fixture `CloudSource` serving it, in the shape `engine.rs`'s `VariantCloud` already has.
   `base_params` sets `cloud_opacity: 0.0` for the same reason it sets
   `milky_way_intensity: 0.0`: otherwise a layer that covers half the frame moves all twelve
   existing references and buries what each of them is for. Waiting for it uses the existing
   `wait_for_slot_texture("cloud_texture")`.
2. **Two goldens across the terminator**, since one hemisphere alone would pass with either
   half of this reverted. A framing with the terminator down the middle pins the floor, the
   shift and the width at once, and it is the framing that has to survive being looked at by
   eye, which is what the golden suite is for.
3. **An engine case for the ordering claim**, which is what a golden cannot state: a
   night-side cloud pixel is brighter than the unlit surface under it. That is the defect in
   one assertion and it fails on the current code.
4. **Three engine cases for the sentinel**, one per single-texture mode, asserting that a
   dayside cloud pixel is brighter than a night-side one. Each fails today.
5. **A golden for the city glow**, in blend mode over the night hemisphere with a night
   fixture that has a bright patch in a known place, so the reference shows the deck
   brightening over the patch and not elsewhere. One case, because the effect is one term.
6. **An engine case that zero is off**, rendering the same scene at `cloud_city_gain` of
   zero and comparing byte for byte against the same scene with the night texture absent.
   That is what D7's "zero has to mean gone" asserts, and it is the case that fails if the
   branch is replaced by a multiply.
7. **An engine case across the resolution switch** with both a cloud source and the night
   slot loaded, asserting a frame still arrives after the purge. Without the two rebuilds
   part two describes, that is a validation error rather than a wrong pixel, so the case
   fails loudly. `tests/engine.rs` already has the harness for it.
8. The digest table entries for `cloud_night` and `cloud_city_gain` come free from
   `params.rs`'s existing walk, which is what makes forgetting the dirty check a test
   failure.

## Risks

The night hemisphere turning into gray fog is the real one, and it is the failure Elite
Dangerous's forums describe. The mitigation is that the default is chosen against the actual
texture it draws over rather than against a photograph shot at a different exposure, and
that the control exists so the judgment is the user's. It still wants one look at a
real frame with the real assets before the number is committed, which is a thing no test can
do; the two goldens are what keep it from drifting afterwards.

The second risk belongs to part two: the cloud bind group now spans two slots with different
lifetimes, and the purge is the place that gets it wrong. A missed rebuild is a validation
error on the next draw rather than a wrong pixel, which is why test 7 exists and why part
two goes second. Part one carries none of it.

The third is that the city glow is an invention rather than a citation. It is defensible as
physics, since upwelling light does reach cloud bases and the Day/Night Band shows the
result, but the transfer function and the blur scale are both chosen rather than derived.
The control is what keeps that honest: a user who thinks it overdone has a slider, and zero
returns the layer to the part one behavior exactly.

## Sources

Read by a subagent on 2026-08-29 except the last three, which were fetched and measured
here.

- [BAFact Math: The Sun is 400,000 times brighter than the full Moon](http://blogs.discovermagazine.com/badastronomy/2012/08/27/bafact-math-the-sun-is-400000-times-brighter-than-the-full-moon/)
- [Orders of magnitude (illuminance)](https://en.wikipedia.org/wiki/Orders_of_magnitude_(illuminance))
- [How bright is moonlight?, Astronomy & Geophysics](https://academic.oup.com/astrogeo/article/58/1/1.31/2938119)
- [A multi-band map of the natural night sky brightness](https://arxiv.org/pdf/2101.01500)
- [VIIRS Day/Night Band imagery: city lights and cloud-top lightning illumination, CIMSS](https://cimss.ssec.wisc.edu/satellite-blog/archives/11454)
- [Best Clouds for Sunsets: The Science of Red Skies](https://www.absurdlyoptimized.com/outdoors/sunsets/) (terminator dip angle, screening height)
- [civil twilight, Glossary of Meteorology](https://glossary.ametsoc.org/wiki/civil-twilight/)
- [Daylight and Twilight Explained, Visual Crossing](https://www.visualcrossing.com/resources/documentation/weather-data/daylight-civil-nautical-astronomical-twilight/)
- [Cameras vs. The Human Eye, Cambridge in Colour](https://www.cambridgeincolour.com/tutorials/cameras-vs-human-eye.htm)
- [Physically Based Sky, Atmosphere and Cloud Rendering in Frostbite, Hillaire, SIGGRAPH 2016](https://www.frostbite.com/frostbite/news/physically-based-sky-atmosphere-and-cloud-rendering)
- [The Real-time Volumetric Cloudscapes of Horizon Zero Dawn, SIGGRAPH 2015](https://advances.realtimerendering.com/s2015/The%20Real-time%20Volumetric%20Cloudscapes%20of%20Horizon%20-%20Zero%20Dawn%20-%20ARTR.pdf)
- [Art-Directable Multiple Volumetric Scattering, Wrenninge](https://history.siggraph.org/learning/art-directable-multiple-volumetric-scattering-by-wrenninge/)
- [Volumetric Clouds, jpg's blog](https://www.jpgrenier.org/clouds.html) (ambient function, Beer-Powder)
- [Henyey-Greenstein phase function](https://en.wikipedia.org/wiki/Henyey%E2%80%93Greenstein_phase_function)
- [HDR rendering, SpaceEngine dev blog](https://spaceengine.org/news/blog170312/)
- [What's up with the dark side of planets?, Elite Dangerous discussion](https://steamcommunity.com/app/359320/discussions/0/1290691308582293715/)
- [Hello, World (photograph)](https://en.wikipedia.org/wiki/Hello,_World_(photograph)) and its [Commons file](https://commons.wikimedia.org/wiki/File:Earth_From_the_Perspective_of_Artemis_II.jpg), measured
- [A tale of two photos: Artemis II's Hello World and Apollo 17's Blue Marble, RedShark News](https://www.redsharknews.com/artemis-ii-hello-world-nikon-d5-blue-marble-comparison) (the Apollo exposure)
- `textures/BlackMarble_2016.jxl` in this repository, measured

One thing the subagent could not verify and flagged: the multiple-scattering octave
coefficients circulating in community write-ups do not trace to a primary source. No number
from them is used here.
