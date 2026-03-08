# Live Earth Wallpaper - Technology Decisions

## Constraints

1. **Cross-platform** - Windows 11 is the priority, Linux and macOS are secondary targets.
2. **Desktop GUI** - Users need a settings window to configure camera position, update interval, and other parameters. Should feel native and responsive.
3. **3D rendering** - Must render a textured sphere (Earth) with multiple layers: base texture (Blue Marble), cloud overlay, night lights, day/night terminator. Later: moon, stars, planets in astronomically correct positions.
4. **Background operation** - Runs as a tray/background app, periodically renders an image and sets it as the desktop wallpaper. Must be lightweight (low CPU/RAM when idle).
5. **Wallpaper integration** - Needs OS-specific APIs to set the desktop background image.
6. **Astronomical accuracy** - Sun position (for day/night), later moon/planet positions, must be computed from current date/time and be accurate.

---

## Setting the Desktop Wallpaper (per OS)

This is straightforward on all platforms - render to an image file, then call the OS API:

- **Windows:** `SystemParametersInfo(SPI_SETDESKWALLPAPER, ...)` from `user32.dll`. Well-documented, works from any language via FFI.
- **Linux:** Depends on desktop environment:
  - GNOME/Cinnamon/MATE: `gsettings set org.gnome.desktop.background picture-uri file:///path/to/image`
  - KDE Plasma: DBus call to `org.kde.PlasmaShell.evaluateScript`
  - X11 fallback: `feh --bg-scale`
- **macOS:** `osascript` or `NSWorkspace` API.

This part is not a major technology driver - any language can shell out or FFI into these APIs.

---

## Astronomical Calculations

We need a library that can compute the position of the sun (for day/night), moon, and planets for any given date/time. Later features like star field rendering and eclipse prediction are a bonus. Accuracy within a few arcminutes is sufficient - we're rendering wallpapers, not navigating spacecraft.

### Astronomy Engine

- **Repository:** https://github.com/cosinekitty/astronomy
- **Languages:** C, C#, Python, JavaScript, Kotlin
- **License:** MIT
- **Accuracy:** ~1 arcminute, based on VSOP87 and NOVAS C 3.1 models
- **Size:** C source is a single file pair (astronomy.h + astronomy.c). JS minified is ~116KB.
- **Features:** Sun, Moon, all planets (Mercury-Pluto), lunar phases, eclipses, transits, oppositions, conjunctions, equinoxes, solstices, rise/set times, coordinate transforms (equatorial, ecliptic, horizontal, galactic).
- **Dependencies:** None (pure standard library for each language)
- **Activity:** 827 stars. 2,242 commits. Last release v2.1.19 (Dec 2023). Issues and discussions active through 2025. The author states: "I am committed to maintaining this project for the long term." The project appears mature and stable rather than abandoned - the core algorithms don't change often, but the maintainer is responsive.
- **Assessment:** The best all-around choice. Covers all our current and future needs. Well-documented, well-tested, actively maintained. The single-file C implementation makes FFI straightforward.

**Rust compatibility:** An `astronomy-engine-bindings` crate exists on crates.io (v2.1.19, ~1,143 downloads, 1 version published). It uses bindgen to auto-generate Rust FFI bindings from the C header. This is a thin wrapper - it exposes the raw C API, so calls would be `unsafe`. The crate tracks upstream versions. Requires clang installed for bindgen at build time. The low download count suggests limited community testing, but since it's auto-generated bindings over a well-tested C library, the risk is low. The main ergonomic cost is writing safe Rust wrappers around the `unsafe` FFI calls, which is a one-time effort given the API surface is manageable.

### astro (astro-rust)

- **Repository:** https://github.com/saurvs/astro-rust
- **Language:** Pure Rust
- **License:** MIT
- **Accuracy:** Based on Jean Meeus' "Astronomical Algorithms" book. Uses VSOP87 for planets, ELP-2000/82 for the Moon.
- **Size:** ~80K downloads on crates.io, 10 versions published
- **Features:** Planetary positions (Mercury-Pluto via VSOP87), lunar position (ELP-2000/82), Julian dates, sidereal time, dynamical time, equinoxes, rise/set times, coordinate transforms.
- **Dependencies:** Pure Rust, no external dependencies
- **Activity:** 304 stars. Only 14 commits total. Still uses Travis CI (defunct). No activity in ~8 years. **Effectively abandoned.**
- **Assessment:** Dead project. Despite being the most downloaded pure-Rust astronomy crate, it hasn't been touched in years. The algorithms are based on well-known references (Meeus) so the code likely still works, but there are no bug fixes, no Rust edition updates, and it may not compile cleanly with modern Rust toolchains. Not recommended for new projects.

### vsop87 (vsop87-rs)

- **Repository:** https://github.com/Razican/vsop87-rs
- **Language:** Pure Rust
- **License:** MIT + Apache 2.0
- **Accuracy:** Under 1 arcsecond for 4,000 years around J2000 (Mercury-Mars), 2,000 years (Jupiter-Saturn), 6,000 years (Uranus-Neptune).
- **Features:** All VSOP87 algorithm variants (A through E). Heliocentric positions for all planets in various coordinate systems.
- **Activity:** 18 stars. Last release v3.0.0 (Oct 2023). **Archived by owner on Feb 3, 2026** - now read-only. Not dead in the same way as astro-rust (it received updates until recently), but no further development will happen.
- **Assessment:** The code is complete and correct for what it does (VSOP87 is a fixed algorithm), so being archived is less concerning than for a feature-evolving project. But it's VSOP87-only - no moon, no sun position, no eclipses, no rise/set times, no coordinate transforms. A building block, not a complete library. Combining it with other crates to get a full solution would be fragile given the archived status.

### Swiss Ephemeris

- **Website:** https://www.astro.com/swisseph/swisseph.htm
- **Repository:** https://github.com/aloistr/swisseph
- **Language:** C (with bindings for many languages)
- **License:** AGPL or commercial license
- **Accuracy:** 0.001 arcsecond (milli-arcsecond) - agrees with JPL Ephemeris. By far the most accurate option.
- **Features:** Sun, Moon, planets, 300,000+ asteroids, fixed stars, eclipses, occultations, rise/set/transit times, heliacal risings. The industry standard for professional astronomical/astrological software.
- **Activity:** 596 stars. 748 commits. README last updated Apr 2025. Last tagged release v2.10.03 (Sep 2022), but commits continue. Maintained by Astrodienst AG (Swiss company), with a public mailing list. **Still actively maintained**, though release cadence is slow.
- **Assessment:** Very capable but heavy for our use case. AGPL license means the entire application must be open-sourced if distributed (fine if publishing as free software). Requires external ephemeris data files. The API is complex and C-oriented. Accuracy is far beyond what we need for wallpaper rendering. Could be a good choice if we want to build a "serious" astronomy tool, but overkill for the initial scope.

### libnova

- **Website:** https://libnova.sourceforge.net/
- **Repository:** https://github.com/JohannesBuchner/libnova
- **Language:** C
- **License:** LGPL
- **Features:** Aberration, nutation, precession, proper motion, sidereal time, solar/planetary/lunar positions (VSOP87 + ELP82), coordinate transforms, magnitude/phase calculations.
- **Activity:** 21 stars. **Archived on Jun 26, 2023** - the GitHub repo is marked as "local, outdated copy" and is now read-only. The original SourceForge project also appears dormant. **Dead project.**
- **Assessment:** Do not use. Archived, unmaintained, and the GitHub copy is explicitly labeled as outdated. Astronomy Engine covers the same scope with better documentation, more languages, and active maintenance.

### Skyfield (Python only)

- **Website:** https://rhodesmill.org/skyfield/
- **Language:** Python (depends on NumPy)
- **Accuracy:** Sub-milliarcsecond, validated against US Naval Observatory and NASA JPL HORIZONS
- **Features:** Stars, planets, Earth satellites, comets, asteroids. Research-grade.
- **Activity:** Actively maintained, regular releases.
- **Assessment:** Excellent library but Python-only with NumPy dependency. Not practical for a compiled desktop app unless using Python as the core language. Mentioned for completeness.

### SunCalc

- **Repository:** https://github.com/mourner/suncalc
- **Languages:** JavaScript (ports exist for many languages including Rust: `suncalc` crate)
- **Features:** Sun position, sun phases (sunrise, sunset, etc.), moon position, moon illumination. Very basic - no planets.
- **Activity:** 3,300 stars. Actively maintained (recent ESM modernization commit by the author). Well-established project by Vladimir Agafonkin (also created Leaflet).
- **Assessment:** Too limited for our needs. Good for sun/moon only but we need planets for the future roadmap. The Rust `suncalc` crate is a port but unclear how well maintained it is.

### Comparison

| Library | Language | Accuracy | Planets | Moon | Eclipses | License | Maintained | FFI (from Rust) |
|---|---|---|---|---|---|---|---|---|
| Astronomy Engine | C/C#/JS/Py/Kt | ~1 arcmin | All | Yes | Yes | MIT | Yes (active) | Yes (bindings exist) |
| astro-rust | Pure Rust | ~arcmin | All | Yes | No | MIT | No (~8yr dead) | No |
| vsop87-rs | Pure Rust | <1 arcsec | All | No | No | MIT/Apache | No (archived) | No |
| Swiss Ephemeris | C | ~0.001 arcsec | All+asteroids | Yes | Yes | AGPL/Commercial | Yes (slow) | Yes |
| libnova | C | ~arcmin | All | Yes | No | LGPL | No (archived) | Yes |
| Skyfield | Python | ~0.0005 arcsec | All+satellites | Yes | Yes | MIT | Yes (active) | N/A |
| SunCalc | JS/Rust | ~arcmin | No | Yes | No | MIT | Yes (active) | No |

### Recommendation

The landscape is clearer with maintenance status considered. Two libraries are actively maintained and feature-complete: **Astronomy Engine** and **Swiss Ephemeris**. Everything else is either dead/archived, too limited, or Python-only.

**If using Rust:** Use `astronomy-engine-bindings` (FFI to C). The pure-Rust alternatives are all dead or incomplete. The FFI cost is a one-time effort. Alternatively, Swiss Ephemeris via FFI if you want maximum accuracy and are fine with AGPL.

**If using C#:** Astronomy Engine is a no-brainer - first-class C# port, native, no FFI, full feature set.

**If using JavaScript:** Same story - Astronomy Engine has a first-class JS port.

**Bottom line:** Astronomy Engine is the clear winner for any language choice. It's the only library that's actively maintained, feature-complete, MIT-licensed, and available in multiple languages natively. Swiss Ephemeris is the alternative if you need extreme precision or asteroid support and are okay with AGPL.

---

## Resource Efficiency, Battery Impact, and GPU Fallback

These concerns cut across all technology choices and are worth addressing separately.

### Idle resource usage

The app spends 99%+ of its time idle (sleeping between renders). What matters:

| | Rust | C# (.NET) | Python |
|---|---|---|---|
| Idle RAM | ~5-15MB (no runtime) | ~25-40MB (CLR + GC heap) | ~40-80MB (interpreter + NumPy) |
| Background CPU | 0% (sleeping) | 0% (sleeping) + occasional GC | 0% (sleeping) |
| Startup overhead | None | JIT compilation on first run | Interpreter + module loading |

Rust wins clearly here. No runtime, no GC, deterministic memory. The .NET GC keeps a minimum heap allocation even when idle. Python is heaviest due to the interpreter and NumPy (Skyfield dependency). In practice, the difference is ~15MB vs ~35MB vs ~60MB for a sleeping background process. All are acceptable, but on a resource-constrained laptop, Rust's frugality is noticeable.

### Rendering efficiency and battery

The rendering happens once every few minutes. Key factors:

**GPU wake-up cost:** Each render wakes the GPU from a low-power state, executes shaders, then lets it sleep again. This is a brief power spike regardless of language. The most battery-friendly approach is to render as infrequently as possible (e.g. every 5-10 minutes rather than every 1 minute) and keep each render fast.

**Vulkan/DX12 vs OpenGL:** Modern APIs (Vulkan, Metal, DX12) have lower CPU overhead than OpenGL, meaning less CPU work per render = less battery drain. Research shows Vulkan can use ~50% less power than OpenGL for equivalent work due to reduced CPU-GPU communication overhead and better multithreading.

**wgpu automatically picks the best backend:** On Windows it prefers DX12, on Mac Metal, on Linux Vulkan. This gives you the most efficient API per platform without any work.

**Practical impact:** For a single textured sphere rendered every 5 minutes, the battery impact is negligible regardless of technology choice. The difference between Rust/wgpu and C#/Silk.NET would be unmeasurable. The update interval setting matters far more than the language.

### Software rendering fallback (no GPU / bad drivers / old hardware)

This is where things get interesting. Options per stack:

**Rust + wgpu:**
- wgpu has a built-in `force_fallback_adapter` option in `RequestAdapterOptions`. When set to `true`, it requests a software renderer.
- **On Windows:** Falls back to WARP (Windows Advanced Rasterization Platform), Microsoft's high-performance software rasterizer built into Windows 10/11. Supports DX12 feature level 12.1. Ships with every Windows installation - no extra download needed.
- **On Linux:** Falls back to Mesa's lavapipe (Vulkan software renderer) or llvmpipe (OpenGL). Available in Mesa, which is installed on virtually all Linux desktops.
- **On Mac:** SwiftShader (Google's Vulkan CPU implementation) can be bundled, though Macs generally have decent GPU support.
- **Implementation:** Try hardware adapter first → if it fails or returns no adapter, retry with `force_fallback_adapter: true`. A few lines of code.
- **Performance with WARP/lavapipe:** Rendering a single textured sphere is trivially simple. Software rendering would complete in under a second. Perfectly fine for a wallpaper that updates every few minutes.

**C# + Silk.NET:**
- **Windows (WARP):** WARP is not automatically selected when hardware GPU creation fails. You must explicitly request it: for D3D11, pass `D3DDriverType.Warp` instead of `D3DDriverType.Hardware`; for D3D12, call `EnumWarpAdapter()`. In practice this is a single enum/call change plus a try/catch around device creation. WARP ships with every Windows 10/11 installation, no extra downloads needed. Note: WARP is DirectX-only - if using Silk.NET's OpenGL bindings, you don't get WARP.
- **Linux (Mesa):** For OpenGL via Silk.NET, Mesa's llvmpipe kicks in automatically if no GPU driver is available - this is transparent at the system level. For Vulkan, lavapipe must be installed (`mesa-vulkan-drivers` package) but then appears as a regular Vulkan device. Effectively automatic for OpenGL, semi-automatic for Vulkan.
- **macOS:** No software rendering path. Apple deprecated OpenGL and there's no Mesa equivalent. MoltenVK translates Vulkan to Metal, but that requires a GPU.
- **Silk.NET WebGPU path:** Silk.NET has WebGPU bindings backed by wgpu-native (`Silk.NET.WebGPU.Native.WGPU` NuGet). This should expose `RequestAdapterOptions.ForceFallbackAdapter`, giving the same one-flag experience as Rust's wgpu. However, the .NET WebGPU bindings are less battle-tested.
- **Alternative: pure-CPU raycasting.** For our specific use case (a single textured sphere saved as an image), GPU rendering can be bypassed entirely. A CPU raycaster using ImageSharp or SkiaSharp computes ray-sphere intersections, maps to UV coordinates on the equirectangular texture, and writes pixels directly. This is ~50-100 lines of math, works everywhere with zero GPU dependency, and runs in tens of milliseconds with `Parallel.For` on a modern CPU. This could serve as a robust fallback mode.
- **Avalonia's own fallback:** Avalonia's UI rendering (separate from 3D) has explicit software rendering support. `Win32PlatformOptions.RenderingMode` accepts an ordered list like `[Vulkan, AngleEgl, Software]` and falls through automatically. The UI itself will always render, even on GPU-less systems.

**Python + ModernGL:**
- ModernGL uses OpenGL Core. On systems with broken GPU drivers, Mesa's llvmpipe provides software OpenGL.
- No explicit "fallback adapter" API - you get whatever the system's OpenGL driver is. If that's llvmpipe, it just works (slowly).
- Least control over the fallback path, but also least effort needed - it "just works" on most Linux systems.

**Alternative approach: pure CPU rendering (no GPU at all):**
- For maximum compatibility, skip the GPU entirely and render on the CPU using a software rasterizer or even ray-marching.
- A single textured sphere is geometrically trivial. A basic CPU renderer in Rust could produce a 1920x1080 frame in well under a second.
- Libraries: `tiny-skia` (Rust 2D rasterizer), `raqote` (Rust), or even custom code.
- This eliminates ALL GPU/driver concerns but limits future rendering complexity (multiple light sources, atmosphere effects, etc. become expensive on CPU).
- Could be a good "safe mode" fallback while keeping GPU rendering as the default.

### Summary: which stack handles these concerns best?

| Concern | Rust + wgpu | C# + Silk.NET | Python + ModernGL |
|---|---|---|---|
| Idle RAM | Best (~5-15MB) | OK (~25-40MB) | Worst (~40-80MB) |
| Battery during render | Best (native, no overhead) | Good (thin GC overhead) | OK (interpreter overhead) |
| GPU fallback (Windows) | Built-in (WARP via wgpu) | WARP via DX (one enum change) or WebGPU flag | Automatic (Mesa) |
| GPU fallback (Linux) | Built-in (lavapipe via wgpu) | Automatic for OpenGL (Mesa), semi-auto for Vulkan | Automatic (Mesa) |
| CPU-only fallback | Easy (tiny-skia, custom) | Easy (raycaster + ImageSharp/SkiaSharp, ~100 lines) | Possible (Pillow/Cairo) |
| Control over rendering | Full | Full | Moderate |

**Rust + wgpu wins on all efficiency and fallback criteria.** The `force_fallback_adapter` API makes software rendering a first-class feature with zero extra libraries. Combined with the lowest idle memory and no runtime overhead, it's the most laptop/battery-friendly option.

---

## Technology Options

### Option A: Rust + wgpu + GUI framework

**Rendering:** wgpu (22k stars, WebGPU-based, translates natively to Vulkan/Metal/DX12/OpenGL). Pure Rust, production-ready, actively maintained. This is the most modern cross-platform graphics abstraction available in any language. Used by Firefox, Bevy, Fyrox, and many other production projects.

**Rendering performance:** For our use case, performance differences between Rust/wgpu and C#/Silk.NET are **irrelevant**. We render one frame every few minutes, not 60fps. A single textured sphere with overlays takes milliseconds to render on any modern GPU regardless of language. The GPU does the heavy lifting via shaders - the CPU-side language barely matters. Where Rust does help: lower idle memory usage (no GC, no runtime), faster startup, and smaller binary. But rendering quality/speed? Identical - same shaders, same GPU.

**3D embedding in GUI:** Both egui and iced support custom wgpu rendering:
- **egui:** Has an official `custom3d_wgpu` example. You render your 3D scene to a texture via `Shape::Callback` or `ui.image()`. Well-documented, working examples exist.
- **iced:** Has `iced_wgpu` with custom rendering support via `shader::Pipeline`. More structured but embedding is less straightforward than egui.

**Astronomy:** `astronomy-engine-bindings` crate (FFI to C). See astronomy section above.

**GUI framework options (deep dive):**

The Rust GUI situation in 2025/2026 has improved but is still behind mature ecosystems. Here are the realistic options:

**egui** (22k stars, very actively maintained by emilk)
- Immediate-mode GUI, renders via wgpu. Zero dependencies on native toolkits.
- Looks functional/clean but NOT native. Think "developer tool" aesthetic, not "polished desktop app".
- Excellent for: settings panels with sliders, dropdowns, checkboxes, color pickers - exactly what we need.
- 3D embedding is well-supported with official examples.
- Smallest binary of any option.
- Limitations: no screen reader accessibility, partial IME support (problematic for CJK input), limited layout system compared to XAML/HTML.
- Used by Rerun (data visualization tool) in production.
- **Best fit for our project** if going Rust - we need a simple settings UI, not a complex application.

**iced** (27k stars, actively maintained)
- Elm-inspired retained-mode GUI. More structured than egui (model-update-view pattern).
- Used by System76 for COSMIC desktop environment (Pop!_OS) - serious production use.
- Better for complex, stateful UIs than egui.
- BUT: no accessibility (screen reader) support, no IME support, 3D widget embedding is less straightforward.
- Overkill for a settings-panel-style app.

**Slint** (19k stars, backed by SixtyFPS GmbH)

Slint is architecturally different from egui/iced. You write UI in a declarative DSL (`.slint` files, inspired by QML) that gets compiled to native code. Business logic is in Rust (or C++/JS/Python). The DSL describes layout, styling, animations, and property bindings declaratively. A reactive property system auto-updates the UI when data changes.

*How it works:*
- `.slint` file defines the UI (components, layout, styling, animations)
- Rust code creates the window, sets property values, handles callbacks
- Properties are reactive: change a value in Rust → UI updates automatically
- Compiler turns `.slint` into native code (no interpreter at runtime)

*Rendering backends (choose at compile time):*
- **Skia renderer** - GPU-accelerated (OpenGL, Vulkan, Metal, DX). Best quality. Heavy disk footprint (~15-20MB added).
- **FemtoVG renderer** - GPU-accelerated via OpenGL ES 2.0. Lighter than Skia. Can also use wgpu (Vulkan/Metal/DX) with `renderer-femtovg-wgpu` feature.
- **Software renderer** - CPU-only, no GPU needed at all. Works anywhere. Good for embedded/fallback.
- **Qt renderer** - Uses system Qt for native widget look (Linux only, requires Qt installed).

This is notable: Slint has a **built-in software renderer** that works with zero GPU dependencies. Combined with wgpu's `force_fallback_adapter`, this gives two levels of fallback.

*3D rendering integration (the key question):*

Slint 1.12 (2025) added **official wgpu integration** via the `slint::wgpu_28` module. Three approaches for embedding 3D:

1. **wgpu texture import** (recommended for our use case): Render your 3D scene to a wgpu texture, then import it as a `slint::Image` via `Image::try_from()`. The Slint UI displays it like any other image. Clean separation between 3D rendering and UI.

2. **Rendering notifier (underlay/overlay)**: Use `Window::set_rendering_notifier()` to get callbacks with access to the wgpu `Device` and `Queue`. Render your 3D scene as an underlay (behind Slint UI) or overlay (on top). Used in the official Bevy integration example.

3. **OpenGL texture**: Render to an OpenGL texture, import via `Image::from_borrowed_gl_2d_rgba_texture()`. Works with the FemtoVG renderer. Official `opengl_texture` and `opengl_underlay` examples exist.

For our use case, approach #1 is ideal: render the Earth to a texture, display it in the settings window as a preview, and also save it to disk as the wallpaper. The 3D rendering is completely decoupled from Slint's UI rendering.

*Licensing:*
- **GPLv3** - free for open source (fine if publishing as free software)
- **Royalty-free license** - free for proprietary desktop/mobile/web apps
- **Commercial license** - for proprietary embedded apps

*Tooling:* VS Code extension with live preview, Figma import, hot reload for `.slint` files.

*Strengths over egui for this project:*
- Best accessibility of any Rust GUI (screen reader support)
- Best IME support (important for international users)
- Declarative UI is easier to maintain than immediate-mode
- Built-in software renderer as fallback
- Official wgpu integration with examples
- More polished/professional look than egui
- Hot reload for UI development

*Weaknesses:*
- DSL is another language to learn (though it's simple and QML-inspired)
- Skia renderer adds ~15-20MB to binary
- Less Rust-idiomatic than egui (DSL + Rust rather than pure Rust)
- Smaller community than egui
- Company-backed: risk of licensing changes (though GPLv3 is irrevocable)

**Dioxus** (24k stars, actively maintained, small funded team)
- React-like framework. Desktop mode uses WebView2 (Windows) / WebKit (Linux/Mac).
- Good accessibility (Narrator works), good IME support.
- Desktop rendering uses a webview - same architectural concerns as Tauri (WebGL in webview for 3D).
- Best for web-first or if you know React patterns. Not ideal for native 3D rendering.

**Summary of Rust GUI landscape:**

| Framework | Stars | Style | 3D integration | Accessibility | Native look | Maturity | Software fallback |
|---|---|---|---|---|---|---|---|
| egui | 22k | Immediate mode | Good (wgpu) | Poor | No (custom) | Stable | Via wgpu |
| iced | 27k | Elm/retained | Possible (wgpu) | Poor | No (custom) | Stable | Via wgpu |
| Slint | 19k | DSL/declarative | Good (wgpu + OpenGL) | Good | Yes (Qt backend) | Production | Built-in SW renderer |
| Dioxus | 24k | React-like | Webview only | Good | Webview-based | Stable | N/A |

For our specific needs (settings panel + 3D preview + efficiency + fallback), **Slint is now the stronger Rust GUI choice**:
- Official wgpu integration (since v1.12) with multiple 3D embedding approaches
- Built-in software renderer for systems without GPU
- Best accessibility and IME support
- More polished look than egui
- The DSL is extra complexity, but the `.slint` files are simple for a settings panel

**egui remains a solid alternative** if you want pure Rust with no DSL and the simplest possible setup. The "developer tool" aesthetic is fine for a wallpaper settings panel.

**Pros (updated):**
- Single static binary, ~5-15MB, no runtime dependencies
- Lowest memory footprint (~10-30MB idle)
- wgpu is production-ready and the most modern graphics abstraction
- egui has proven wgpu+3D integration with official examples
- True cross-platform from a single codebase - `cargo build` and you're done
- Fastest startup, smallest download for users
- Satpaper (existing tool) proves the Rust + wallpaper approach works well
- No GC pauses, no runtime overhead - ideal for a background process

**Cons (updated):**
- Rust has a steep learning curve (but you write it once)
- egui looks functional but not native - fine for a settings panel, might disappoint users expecting native UI
- No accessibility support (screen readers won't work with egui)
- Astronomy Engine requires FFI to C (bindings crate exists, one-time wrapper effort)
- Slower iteration speed compared to C#/Python
- If you later want a more polished/native-looking GUI, you'd need to switch frameworks

---

### Option B: C# + Avalonia UI + 3D Rendering

**Rendering - the .NET 3D landscape in detail:**

The .NET graphics library landscape needs careful evaluation. The situation is more nuanced than initially assessed.

**Silk.NET** (4.9k stars) is a .NET Foundation project providing bindings for OpenGL, Vulkan, DirectX, WebGPU, SDL, GLFW, and more. Last release v2.23 "Winter 2025 Update" (Jan 2026). Actively maintained. However, Silk.NET is fundamentally a **low-level bindings library, not a rendering engine**. It provides:
- **Raw API bindings:** Direct 1:1 C# wrappers over OpenGL/Vulkan/DX/WebGPU calls. You call `gl.BindBuffer(...)`, `gl.TexImage2D(...)` etc. exactly as you would in C/C++.
- **Platform abstractions:** Higher-level wrappers for windowing (`Silk.NET.Windowing`) and input (`Silk.NET.Input`).
- **No scene graph, no mesh primitives, no material system, no built-in 3D objects.** It does not generate sphere geometry, manage render passes, or handle texture loading (you need third-party libraries like StbImageSharp).
- **Headless/offscreen rendering is awkward.** Official EGL bindings were dropped in v2.0 and won't return until at least v3.0. Workaround: render to a framebuffer object (FBO) within a hidden window.
- Silk.NET v3.0 is a planned major rewrite but has no firm release date; many features (including WebGPU bindings regeneration) are listed as "Not Started."

Rendering a textured sphere with Silk.NET (OpenGL path) requires ~250-350 lines of manual boilerplate: writing GLSL shaders as strings, compiling/linking them, generating sphere vertices with trigonometry, creating VBOs/VAOs/EBOs, loading textures via a third-party library, setting up projection matrices, and writing the render loop. This is roughly comparable to raw wgpu in Rust in volume, but wgpu provides a safer, more modern abstraction (render passes, bind groups, pipeline descriptors) while Silk.NET's OpenGL path is the classic stateful OpenGL model. Using Silk.NET's Vulkan bindings instead would be significantly worse (1000+ lines easily).

Silk.NET also has WebGPU bindings (`Silk.NET.WebGPU.Native.WGPU` NuGet package) backed by wgpu-native. This would be the closest equivalent to Rust's wgpu, including `RequestAdapterOptions.ForceFallbackAdapter` for software rendering. However, the .NET WebGPU ecosystem is less battle-tested than the OpenGL/Vulkan/D3D bindings.

**Veldrid** (2.7k stars) was the previously recommended choice - it provided a higher-level abstraction over multiple backends (D3D, Vulkan, Metal, OpenGL), similar in concept to wgpu. But the maintainer announced in Feb 2023 they can no longer publicly share updates. Last release v4.9.0 (Feb 2023). **Not recommended for new projects.**

**OpenTK** is actively maintained and v5 is in development with Vulkan support, but it's primarily an OpenGL binding - and OpenGL is deprecated on macOS.

**Ab4d.SharpEngine** is a higher-level alternative worth noting:
- Vulkan-based 3D engine with a scene graph and built-in primitives including `SphereModelNode`.
- A textured sphere would be ~20-30 lines of code instead of 250-350 with Silk.NET.
- Explicit offscreen/headless rendering support, including software rendering via llvmpipe.
- Cross-platform: Windows, Linux, macOS.
- Avalonia integration available.
- **Licensing:** Commercial, but free for open-source projects. Actively maintained (v3.2, 2025-2026 releases).
- Adds a dependency on a commercial (if open-source-friendly) library. The project is maintained by a small team - long-term risk if the company discontinues it.

**Other .NET 3D options considered:**
- **Stride** (formerly Xenko): Full open-source game engine (.NET Foundation, MIT). Overkill for a textured sphere, but usable in "code-only" mode. Headless rendering not well-documented.
- **MonoGame**: Simpler than raw Silk.NET (`BasicEffect` with texture support, `RenderTarget2D` for offscreen). Still need to generate sphere geometry. Headless mode poorly supported.
- **Helix Toolkit**: WPF/Avalonia 3D component library. Requires a visible window for rendering (can't do offscreen easily). v3 under development.

**Summary of .NET 3D options:**

| Library | Level | Boilerplate | Offscreen | Software fallback | Maintained | License |
|---|---|---|---|---|---|---|
| Silk.NET (OpenGL) | Raw bindings | ~300 lines | Hacky (hidden window) | Via Mesa/llvmpipe | Yes | MIT |
| Silk.NET (WebGPU) | Raw bindings | ~300 lines | Likely possible | force_fallback_adapter | Less tested | MIT |
| Ab4d.SharpEngine | Scene graph | ~30 lines | Built-in | Built-in (llvmpipe) | Yes | Commercial (free for OSS) |
| MonoGame | Game framework | ~150 lines | Poor | Via Mesa | Yes | MIT |
| Stride | Full engine | ~100 lines | Poor | Unknown | Yes | MIT |

For our use case (render a textured sphere with multi-layer overlays every few minutes), the honest picture is: **Silk.NET is significantly more low-level than initially portrayed**, roughly equivalent to writing raw OpenGL/Vulkan. Ab4d.SharpEngine would dramatically reduce boilerplate but adds a commercial dependency. There is no .NET equivalent of wgpu's abstraction level that is both mature and actively maintained (that was Veldrid's niche, and it's gone).

**GUI:** Avalonia UI (WPF-inspired XAML framework, truly cross-platform: Windows/Linux/macOS).
- 27k+ stars on GitHub, very actively developed (latest: v11.3.12)
- Renders its own UI (doesn't use native controls) - looks consistent across platforms but not perfectly "native"
- Recently adding Wayland support for modern Linux desktops (XWayland works as fallback today)
- Moving from Skia to Impeller (Google's Flutter renderer) via "Nimpeller" for better performance
- Official Debian/Ubuntu packaging documentation exists
- .NET MAUI announced Linux support via Avalonia in late 2025, validating the framework

**Astronomy:** Astronomy Engine has a first-class C# port.

**Linux deployment (the "1GB of packages" question):**

The situation has improved significantly since the early .NET Core days:
- **Self-contained publish** bundles the .NET runtime into your app. Users don't need to install .NET separately. No Microsoft package repos required.
- **Typical size:** ~60-80MB untrimmed, ~30-50MB with trimming enabled. Single-file publish produces one executable.
- **Native dependencies:** A self-contained .NET app on Linux still needs a few system libraries: `libc`, `libgcc`, `libstdc++`, `libssl`, `libicu` (or opt out of globalization with `InvariantGlobalization=true`). These are standard and come pre-installed on virtually every Linux desktop distribution.
- **Distribution options:** Can be packaged as AppImage, Flatpak, Snap, or .deb. Avalonia has official docs for Debian/Ubuntu packaging.
- **No foreign repos.** You do NOT need to add Microsoft's package repository. Self-contained means everything is bundled. Users download one file, run it.

**Pros:**
- Astronomy Engine works natively in C# - no FFI needed
- Avalonia is the most mature cross-platform .NET GUI framework (27k stars, very active)
- Avalonia has built-in software rendering fallback for its UI (tries GPU first, falls through to CPU automatically)
- XAML-based UI is powerful and flexible for settings screens
- Fast iteration speed, good tooling (IDE support, hot reload)
- .NET ecosystem is rich (JSON config, HTTP for cloud data, etc.)
- EarthLiveSharp (existing C# tool) is a good reference for Windows wallpaper integration
- C# is widely known, easier to attract contributors
- Self-contained Linux deployment is straightforward, no .NET runtime installation needed
- CPU raycasting fallback (ImageSharp/SkiaSharp) is straightforward (~100 lines of math) and eliminates all GPU concerns
- WARP fallback on Windows is a one-line change (enum swap in D3D device creation), no extra downloads needed
- On Linux, OpenGL via Mesa automatically falls back to llvmpipe if no GPU is present

**Cons:**
- Self-contained binary is ~30-80MB depending on trimming (much larger than Rust, comparable to Python)
- **Silk.NET is raw API bindings, not a rendering engine.** Rendering a textured sphere requires ~250-350 lines of manual OpenGL/Vulkan boilerplate (shaders, buffers, geometry generation, texture loading). No scene graph, no built-in primitives. This is comparable to writing raw OpenGL in C/C++.
- **Veldrid is gone.** The library that provided wgpu-like abstraction in .NET is semi-abandoned since Feb 2023. Nothing has replaced it at the same abstraction level. This is the biggest gap in the .NET 3D ecosystem.
- Ab4d.SharpEngine would reduce 3D boilerplate to ~30 lines but adds a commercial dependency (free for OSS, but small-team risk)
- OpenGL is deprecated on macOS (Vulkan via MoltenVK or Metal backend needed for future-proofing). No software rendering path on macOS at all.
- Avalonia renders its own UI, not native controls - can feel "off" to users expecting native look
- 3D rendering in Avalonia requires custom control integration (doable but not trivial)
- Avalonia's Wayland support is still WIP (XWayland fallback works but isn't ideal long-term)
- Headless/offscreen rendering with Silk.NET is awkward (EGL bindings dropped in v2.0, workaround: hidden window + FBO)
- Slightly higher memory footprint than native compiled languages (~30-60MB RAM)

---

### Option C: Tauri (Rust backend + Web frontend)

**Rendering:** Three.js or Babylon.js in the webview for 3D globe. Rust backend handles wallpaper setting, file I/O, scheduling.

**GUI:** HTML/CSS/JS (any web framework: React, Svelte, Vue, etc.) rendered in the system webview.

**Astronomy:** Astronomy Engine has a first-class JavaScript port. Could also use the C library from the Rust backend.

**Pros:**
- Beautiful, highly customizable UI using web technologies
- Tauri is lightweight (~5-10MB) compared to Electron
- Astronomy Engine works natively in JS
- Huge ecosystem for web UI components
- Rust backend for performance-critical parts
- Large developer community, easy to find contributors

**Cons:**
- WebGL performance in Tauri's webview is problematic - known lag issues, context loss bugs, and platform-dependent behavior (depends on system webview: Edge WebView2 on Windows, WebKit on Linux/Mac)
- WebGPU support in webviews is still limited/experimental
- Split architecture (Rust + JS) adds complexity
- Webview rendering quality varies across platforms
- For a tool that primarily renders a single image periodically, the web stack may be overkill

---

### Option D: Electron + Three.js

**Rendering:** Three.js (WebGL/WebGPU) for 3D globe rendering. Chromium provides consistent rendering across platforms.

**GUI:** HTML/CSS/JS with any web framework.

**Astronomy:** Astronomy Engine JS port works natively.

**Pros:**
- Most mature cross-platform desktop app framework
- Consistent rendering across all platforms (bundled Chromium)
- Three.js is battle-tested for 3D globe rendering
- Astronomy Engine works natively
- Largest ecosystem and community
- Fastest to prototype and iterate

**Cons:**
- Heavy: ~150-300MB base size, ~100-200MB RAM at idle
- For a background app that renders one image every few minutes, bundling an entire browser engine is excessive
- Chromium updates and security patches are a maintenance burden
- Overkill for the actual workload

---

### Option E: Godot Engine

**Rendering:** Godot's built-in 3D renderer (Vulkan/OpenGL).

**GUI:** Godot's UI system (Control nodes).

**Astronomy:** Would need GDScript/C# implementation or GDExtension binding to C library.

**Pros:**
- Powerful 3D renderer with PBR, lighting, atmosphere shaders
- Built-in scene editor for visual development
- Cross-platform out of the box
- Could produce the most visually impressive result with least custom shader work
- C# scripting available (can use Astronomy Engine C# port)

**Cons:**
- Headless/offscreen rendering is poorly supported in Godot 4 - --headless disables all rendering, and offscreen rendering is still a work-in-progress feature
- Game engine is heavy for a wallpaper app
- Export templates add significant binary size
- Not designed for this use case (background service + wallpaper)
- Would need workarounds for the render-to-file workflow

---

### Option F: Python + PySide6 + ModernGL

**Rendering:** ModernGL (modern Python wrapper over OpenGL Core, clean API, high performance for Python) or PyOpenGL. PySide6 has `QOpenGLWidget` for embedding OpenGL rendering directly in the GUI. Alternatively, render offscreen to an image file using ModernGL's standalone context (no window needed).

**GUI:** PySide6 (official Qt for Python bindings). Mature, feature-rich, native-looking on all platforms. Excellent widget set for settings screens.

**Astronomy:** Skyfield (the best Python astronomy library, research-grade accuracy, actively maintained) or Astronomy Engine's Python port. Both are first-class native Python libraries.

**Wallpaper setting:** `ctypes.windll.user32.SystemParametersInfoW` on Windows, `gsettings`/`dbus` subprocess calls on Linux, `osascript` on macOS. Well-documented, many existing examples. SeenFromSpace (an existing Python project in this space) already does this.

**Distribution:** PyInstaller (~94MB) or Nuitka (~58MB, compiles to C, 2-4x faster startup). Both produce standalone executables. No Python installation required for end users.

**Pros:**
- **Skyfield is the best astronomy library available in any language** - sub-milliarcsecond accuracy, JPL ephemeris data, actively maintained, beautiful API. No other language gets anything close to this quality natively.
- PySide6/Qt is arguably the best cross-platform GUI toolkit, period. Native look and feel, massive widget library, excellent documentation.
- ModernGL + QOpenGLWidget is a proven combination for embedded 3D rendering.
- Fastest development speed of any option. Python's ecosystem for HTTP (cloud data fetching), image processing (Pillow), scheduling, and system integration is unmatched.
- You're an experienced Python developer - this eliminates the learning curve entirely.
- SeenFromSpace already proves the Python + astronomy + wallpaper approach works.
- Rich ecosystem for data handling: downloading cloud overlay images, processing textures, JSON config, etc.
- Offscreen rendering is trivial with ModernGL's standalone context - no window needed for the background render loop.

**Cons:**
- **Performance ceiling.** Python is ~50-100x slower than C/Rust for CPU-bound work. For rendering a single frame every few minutes this doesn't matter much, but shader setup, texture loading, and astronomical calculations will all be slower. Probably fine in practice but feels wasteful.
- **Distribution is painful.** PyInstaller/Nuitka bundles are large (58-94MB), slow to build, and sometimes brittle across OS versions. Antivirus false positives are a common problem on Windows. Users may get security warnings.
- **Memory footprint.** Python + Qt + OpenGL + NumPy (Skyfield dependency) will use ~80-150MB RAM. Comparable to Electron, worse than Rust/C#.
- **"Not a real app" perception.** Fairly or not, Python desktop apps carry a stigma. Startup is slow (1-3 seconds even with Nuitka), the bundled executable feels like a wrapped script, and platform integration (system tray, autostart, etc.) is second-class compared to native solutions.
- **Qt licensing complexity.** PySide6 is LGPL, which is fine for open source. But Qt's licensing landscape is notoriously confusing and has changed before.
- **OpenGL is deprecated on macOS.** Same issue as the C# option - Apple stopped updating OpenGL. Works today but could break in future macOS versions.
- **NumPy dependency.** Skyfield requires NumPy, which adds to bundle size and build complexity. Not a dealbreaker but adds weight.
- **No existing Python project does 3D globe rendering well.** SeenFromSpace does 2D projections only. PyOpenGLobe exists but is a toy project. You'd still be writing the 3D renderer from scratch, just in a slower language.

---

## Comparison Matrix

| Aspect              | Rust+wgpu  | C#+Avalonia | Tauri     | Electron  | Godot     | Python+Qt  |
|---------------------|------------|-------------|-----------|-----------|-----------|------------|
| Binary size         | ~5-15MB    | ~30-80MB    | ~5-10MB   | ~150-300MB| ~50-100MB | ~58-94MB   |
| RAM (idle)          | ~10-30MB   | ~30-60MB    | ~30-60MB  | ~100-200MB| ~80-150MB | ~80-150MB  |
| 3D rendering        | Excellent  | Adequate*   | Weak      | Good      | Excellent | Adequate   |
| GUI quality         | Basic      | Good        | Excellent | Excellent | Fair      | Excellent  |
| Astronomy lib       | FFI needed | Native      | Native    | Native    | C# or FFI | Skyfield!  |
| Cross-platform      | Excellent  | Good        | Good      | Excellent | Good      | Good       |
| Dev speed           | Slow       | Medium      | Fast      | Fast      | Medium    | Fastest    |
| Maintenance burden  | Low        | Low         | Medium    | High      | Medium    | Medium     |
| Distribution ease   | Excellent  | Good        | Good      | Good      | Fair      | Poor       |
| Linux deployment    | Native bin | Self-cont.  | Native bin| Bundled   | Export    | PyInst/Nui |

---

## Recommendation

**Option A (Rust + wgpu + Slint)** is now the recommended choice.

The deeper research into the C# stack revealed a significant gap: **Veldrid is gone, and nothing has replaced it.** Silk.NET is raw API bindings (~250-350 lines of manual OpenGL boilerplate for a textured sphere), not the higher-level abstraction it was initially portrayed as. Meanwhile, wgpu in Rust provides exactly the abstraction level that Veldrid used to offer in .NET - and it's thriving (22k stars, used by Firefox, actively maintained).

The revised reasoning:

1. **wgpu is the best cross-platform 3D abstraction available in any language right now.** It provides render passes, bind groups, pipeline descriptors, and automatic backend selection (Vulkan/Metal/DX12/OpenGL). The .NET ecosystem has no equivalent since Veldrid died.
2. **`force_fallback_adapter` gives turnkey software rendering** - one flag for WARP on Windows, lavapipe on Linux. The C# equivalent is doable but more fragmented (different approach per API, per platform).
3. **Slint provides a polished, accessible GUI** with official wgpu integration (since v1.12), a built-in software renderer as an additional fallback layer, and a declarative DSL that's well-suited for a settings panel. GPLv3 license is fine for this project.
4. **Single static binary, ~5-15MB, ~5-15MB idle RAM.** The most efficient option for a 24/7 background process.
5. **Astronomy Engine via FFI is a one-time cost.** The `astronomy-engine-bindings` crate exists. Writing safe Rust wrappers around the C API is bounded effort for a manageable API surface. Once done, it's done forever.

The main tradeoff is **development speed** - Rust is slower to iterate in than C#. But the 3D boilerplate is comparable (wgpu vs raw Silk.NET), and the rest of the app (settings UI, wallpaper setting, scheduling) is straightforward in either language.

**egui vs Slint:** Slint is recommended if you plan to distribute to other users (accessibility, polished look, software renderer fallback). egui is fine if you want the simplest possible setup and don't mind the "developer tool" aesthetic.

**Runner-up: Option B (C# + Avalonia UI + Silk.NET/Ab4d.SharpEngine)**

Still a viable choice, especially if:
- You value faster iteration speed over minimal footprint
- You're more comfortable in C# than Rust
- You're willing to use Ab4d.SharpEngine (free for OSS, reduces 3D boilerplate from ~300 lines to ~30)
- Native Astronomy Engine support (no FFI) is important to you

The C# stack's strengths (Avalonia's maturity, native Astronomy Engine, rich ecosystem) are real. Its weakness is specific to 3D: without Veldrid, you're either writing raw OpenGL or depending on Ab4d.SharpEngine. If Ab4d's "free for open source" licensing works for you and you trust the project's longevity, it actually makes the C# option quite competitive.

**The Python case (Option F):** Still a legitimate choice for a personal project. Fastest to prototype, best astronomy library (Skyfield), best GUI toolkit (PySide6/Qt). The downsides (distribution pain, memory, startup time) mostly matter when shipping to others. If you want a working prototype on your own machine fast and worry about distribution later, Python gets you there quickest.

**Avoid:** Electron (too heavy for a background app), Tauri (WebGL issues make 3D unreliable), Godot (offscreen rendering not ready).

\* C# 3D rendering downgraded from "Good" to "Adequate" in the comparison matrix. Silk.NET is raw bindings with no scene graph or built-in primitives - comparable effort to raw OpenGL in C. Ab4d.SharpEngine would bring it back to "Good" but adds a commercial dependency.
