# Research: Slint Testing Backend — Practical Depth (2026-03-24)

## Purpose

The earlier research in `2026-03-22-testability-observability-research.md` established that the
`i-slint-backend-testing` crate exists and can test UI property bindings and callbacks without
a GPU or display. This document provides the practical depth needed to actually write those tests:
exact API signatures, element ID formats, property access patterns, working code templates, version
compatibility information, and a clear map of what the testing backend can and cannot do.

## 1. ElementHandle API in Practice

### Finding elements: `find_by_element_id`

```rust
pub fn find_by_element_id(
    component: &impl ElementRoot,
    id: &str,
) -> impl Iterator<Item = Self>
```

The `id` parameter takes a **qualified ID** in the format `"ComponentName::element-name"`. The
component name is the Slint `export component` name, and the element name is the identifier
assigned with `:=` in the `.slint` file. Both parts are required.

Example: given this `.slint` fragment:

```slint
export component App inherits Window {
    ta := TouchArea { ... }
}
```

You locate it from Rust as:

```rust
let mut it = ElementHandle::find_by_element_id(&app, "App::ta");
let elem = it.next().unwrap();
assert!(it.next().is_none()); // verify exactly one match
```

The element name part uses the **kebab-case identifier** as written in `.slint` (e.g., if the
identifier is `longitude-slider`, the qualified ID is `"MainWindow::longitude-slider"`).

For the Sunlit Earth `.slint`, the export component is `MainWindow`. Thus:

- `"MainWindow::longitude-slider"` — the longitude Slider
- `"MainWindow::diffuse-check"` — the diffuse CheckBox
- `"MainWindow::texture-combo"` — the texture ComboBox

Important limitation: elements inside `for` loops cannot be individually addressed by
`find_by_element_id` because each generated instance does not receive a stable unique qualified ID.
This is a known open feature request (slint-ui/slint#4891, open as of May 2025).

### Finding elements: `find_by_accessible_label`

```rust
pub fn find_by_accessible_label(
    component: &impl ElementRoot,
    label: &str,
) -> impl Iterator<Item = Self>
```

Searches the entire element tree for elements whose `accessible-label` property matches `label`
exactly. For Slint's standard `Button` widget, the `text` property is automatically exposed as the
`accessible-label`, so you can find a button by its display text:

```rust
let buttons: Vec<_> =
    ElementHandle::find_by_accessible_label(&app, "Set as Wallpaper").collect();
assert_eq!(buttons.len(), 1);
buttons[0].invoke_accessible_default_action();
```

For custom elements, an explicit `accessible-label: "..."` property must be set (see section 8).

### ElementQuery builder API

`find_by_element_id` and `find_by_accessible_label` are convenience wrappers. The general query
API is `ElementQuery`, accessible via `ElementHandle::query_descendants()` or
`ElementQuery::from_root()`. This supports chaining:

```rust
// Find all sliders in the component
let sliders = ElementQuery::from_root(&app)
    .match_accessible_role(AccessibleRole::Slider)
    .find_all();

// Find a button by type that also has a specific role
let submit = app
    .root_element()            // → ElementHandle
    .query_descendants()
    .match_inherits("Button")  // matches Button and subtypes
    .match_predicate(|el| el.accessible_label().as_deref() == Some("Submit"))
    .find_first();
```

`ElementQuery` builder methods:

| Method | Signature | Description |
| --- | --- | --- |
| `from_root` | `(component: &impl ElementRoot) -> Self` | Start query at component root |
| `match_descendants` | `(self) -> Self` | Apply subsequent filters to all descendants |
| `match_id` | `(self, id: impl Into<String>) -> Self` | Filter by qualified ID |
| `match_type_name` | `(self, type_name: impl Into<String>) -> Self` | Filter by exact type name |
| `match_inherits` | `(self, type_name: impl Into<String>) -> Self` | Filter by type or any base type |
| `match_accessible_role` | `(self, role: AccessibleRole) -> Self` | Filter by accessible-role |
| `match_predicate` | `(self, predicate: impl Fn(&ElementHandle) -> bool + 'static) -> Self` | Custom filter |
| `find_first` | `(&self) -> Option<ElementHandle>` | Return first match |
| `find_all` | `(&self) -> Vec<ElementHandle>` | Return all matches |

`visit_descendants` is the lower-level traversal primitive:

```rust
pub fn visit_descendants<R>(
    &self,
    visitor: impl FnMut(ElementHandle) -> ControlFlow<R>,
) -> Option<R>
```

### Full ElementHandle API surface (v1.15.1)

Complete list of all methods (no scroll or keyboard methods exist in this version):

**Query and navigation:**

```rust
fn find_by_accessible_label(component: &impl ElementRoot, label: &str) -> impl Iterator<Item = Self>
fn find_by_element_id(component: &impl ElementRoot, id: &str) -> impl Iterator<Item = Self>
fn find_by_element_type_name(component: &impl ElementRoot, type_name: &str) -> impl Iterator<Item = Self>
fn visit_descendants<R>(&self, visitor: impl FnMut(ElementHandle) -> ControlFlow<R>) -> Option<R>
fn query_descendants(&self) -> ElementQuery
```

**Validity and identity:**

```rust
fn is_valid(&self) -> bool
fn id(&self) -> Option<SharedString>                          // → e.g. "MainWindow::longitude-slider"
fn type_name(&self) -> Option<SharedString>                   // → e.g. "Slider"
fn bases(&self) -> Option<impl Iterator<Item = SharedString>> // base types
```

**Geometry:**

```rust
fn size(&self) -> LogicalSize
fn absolute_position(&self) -> LogicalPosition
fn computed_opacity(&self) -> f32
```

**Accessible property readers:**

```rust
fn accessible_role(&self) -> Option<AccessibleRole>
fn accessible_label(&self) -> Option<SharedString>
fn accessible_value(&self) -> Option<SharedString>           // slider current value as string
fn accessible_placeholder_text(&self) -> Option<SharedString>
fn accessible_enabled(&self) -> Option<bool>
fn accessible_description(&self) -> Option<SharedString>
fn accessible_id(&self) -> Option<SharedString>              // accessible-id property
fn accessible_checked(&self) -> Option<bool>                 // checkbox/toggle state
fn accessible_checkable(&self) -> Option<bool>
fn accessible_item_selected(&self) -> Option<bool>
fn accessible_item_selectable(&self) -> Option<bool>
fn accessible_item_index(&self) -> Option<usize>
fn accessible_item_count(&self) -> Option<usize>
fn accessible_expanded(&self) -> Option<bool>
fn accessible_expandable(&self) -> Option<bool>
fn accessible_read_only(&self) -> Option<bool>
fn accessible_value_minimum(&self) -> Option<f32>
fn accessible_value_maximum(&self) -> Option<f32>
fn accessible_value_step(&self) -> Option<f32>
```

**Accessible property writer:**

```rust
fn set_accessible_value(&self, value: impl Into<SharedString>)
// Note: only works if accessible-value is explicitly declared in .slint
```

**Accessible actions:**

```rust
fn invoke_accessible_default_action(&self)  // e.g. clicks a Button
fn invoke_accessible_increment_action(&self) // increments a Slider one step
fn invoke_accessible_decrement_action(&self) // decrements a Slider one step
fn invoke_accessible_expand_action(&self)    // expands a collapsible element
```

**Mouse simulation (async):**

```rust
async fn single_click(&self, button: PointerEventButton)
async fn double_click(&self, button: PointerEventButton)
```

## 2. Property Access from Test Code

The testing backend does **not** expose arbitrary Slint properties via `ElementHandle`. The only
accessible properties are those that map to the `accessible-*` property set in the `.slint`
language.

However, the standard Slint Rust API generated by `slint::include_modules!()` provides direct
`get_<property>()` and `set_<property>()` methods on the component handle for all `in`, `out`,
`in-out`, and property declarations. These work normally with the testing backend:

```rust
// Read a slider value
let lon = app.get_camera_longitude(); // → f32

// Write a slider value
app.set_camera_longitude(42.5);

// Read a checkbox
let is_checked = app.get_diffuse_shading(); // → bool

// Read a ComboBox index
let tex_idx = app.get_texture_index(); // → i32

// Read a string property
let info = app.get_renderer_info(); // → slint::SharedString
```

Property names are converted from kebab-case Slint names to snake_case Rust names automatically
by the code generator. `camera-longitude` → `get_camera_longitude()` / `set_camera_longitude()`.

This is the primary way to read and write properties in tests. The `ElementHandle` accessible
properties are supplementary for finding elements and for asserting accessibility semantics.

### Reading slider range and value via accessible properties

For sliders with `accessible-role: slider`, the accessible value API works:

```rust
let slider = ElementHandle::find_by_element_id(&app, "MainWindow::longitude-slider")
    .next()
    .unwrap();
let val: f32 = slider
    .accessible_value()
    .unwrap()
    .parse()
    .unwrap();
let min = slider.accessible_value_minimum().unwrap(); // → 0.0
let max = slider.accessible_value_maximum().unwrap(); // → 360.0
```

Note that `accessible_value()` returns a `SharedString` and must be parsed to a number. Use
`app.get_camera_longitude()` directly instead where possible.

### Reading a checkbox via accessible properties

For a `CheckBox`, `accessible_checked()` returns the checked state:

```rust
let cb = ElementHandle::find_by_element_id(&app, "MainWindow::diffuse-check")
    .next()
    .unwrap();
assert_eq!(cb.accessible_role(), Some(AccessibleRole::Checkbox));
assert_eq!(cb.accessible_checked(), Some(true));
```

### Combining direct property access and ElementHandle

The cleanest approach in tests is to use direct `get_*`/`set_*` calls for all property reads and
writes, and reserve `ElementHandle` for simulating user actions and asserting accessible semantics:

```rust
// Set up state using direct API
app.set_camera_longitude(-74.0); // New York longitude

// Simulate the user interacting via mouse
let slider = ElementHandle::find_by_element_id(&app, "MainWindow::longitude-slider")
    .next()
    .unwrap();
slider.invoke_accessible_default_action(); // focus

// Verify result via direct API
assert_eq!(app.get_camera_longitude(), -74.0);
```

## 3. Event Simulation

### Mouse clicks

`single_click` and `double_click` are async methods that simulate the full mouse event sequence
(move to center, press, idle period, release). They require an event loop to run:

```rust
use slint::platform::PointerEventButton;

elem.single_click(PointerEventButton::Left).await;
elem.double_click(PointerEventButton::Left).await;
```

The other `PointerEventButton` values are `Right`, `Middle`, and `Other`.

### Invoking accessible actions (synchronous, no event loop needed)

`invoke_accessible_default_action()` activates the element's primary action. For a `Button`, this
is equivalent to clicking it. This is synchronous and works with `init_no_event_loop()`:

```rust
button.invoke_accessible_default_action();
assert!(*was_clicked.borrow());
```

For sliders and spinboxes:

```rust
slider.invoke_accessible_increment_action(); // value += step
slider.invoke_accessible_decrement_action(); // value -= step
```

### Keyboard input

`ElementHandle` has no keyboard simulation methods. The workaround is to dispatch events
directly to the Slint window using `slint::platform::WindowEvent`:

```rust
use slint::platform::{WindowEvent, Key};

// Simulate pressing a key
app.window().dispatch_event(WindowEvent::KeyPressed {
    text: "a".into(),
});
app.window().dispatch_event(WindowEvent::KeyReleased {
    text: "a".into(),
});

// Simulate a special key
app.window().dispatch_event(WindowEvent::KeyPressed {
    text: Key::Tab.into(),
});
```

The `Key` enum (in `slint::platform`) provides constants for special keys: `Tab`, `Return`,
`Escape`, `Backspace`, `Delete`, `UpArrow`, `DownArrow`, `LeftArrow`, `RightArrow`, `F1`..`F24`,
etc.

Note: the older `slint::testing::send_keyboard_string_sequence()` from the pre-1.0 API no longer
exists in the current `i-slint-backend-testing` API surface. Use `window().dispatch_event()`
instead.

### Scroll (wheel) events

Similarly, there is no `scroll()` method on `ElementHandle`. Use `dispatch_event` directly:

```rust
use slint::platform::WindowEvent;

// Scroll 20 logical pixels down at the element's center
let pos = elem.absolute_position();
let size = elem.size();
app.window().dispatch_event(WindowEvent::PointerScrolled {
    position: slint::LogicalPosition::new(
        pos.x + size.width / 2.0,
        pos.y + size.height / 2.0,
    ),
    delta_x: 0.0,
    delta_y: -20.0, // negative = scroll down (platform-dependent convention)
});
```

### Drag events

Drags must be composed from low-level pointer events:

```rust
let center = {
    let p = elem.absolute_position();
    let s = elem.size();
    slint::LogicalPosition::new(p.x + s.width / 2.0, p.y + s.height / 2.0)
};
app.window().dispatch_event(WindowEvent::PointerMoved { position: center });
app.window().dispatch_event(WindowEvent::PointerPressed {
    position: center,
    button: PointerEventButton::Left,
});
let target = slint::LogicalPosition::new(center.x + 50.0, center.y);
app.window().dispatch_event(WindowEvent::PointerMoved { position: target });
app.window().dispatch_event(WindowEvent::PointerReleased {
    position: target,
    button: PointerEventButton::Left,
});
```

### Time control

With `init_no_event_loop()` or `init_integration_test_with_mock_time()`, time is frozen. Use
`mock_elapsed_time()` to advance animations and trigger timers:

```rust
i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(500));
```

## 4. Integration Test Patterns

### Pattern A: Synchronous test without event loop

Use `init_no_event_loop()` when your test does not need async click simulation. This is the
simplest form and suitable for property round-trips and callback verification:

```rust
#[test]
fn test_load_defaults_restores_properties() {
    i_slint_backend_testing::init_no_event_loop();

    let app = MainWindow::new().unwrap();

    // Set some non-default values
    app.set_camera_longitude(123.0);
    app.set_camera_latitude(-45.0);
    app.set_camera_zoom(0.9);

    // Find and activate "Load Defaults" button via accessible label
    let buttons: Vec<_> =
        ElementHandle::find_by_accessible_label(&app, "Load Defaults").collect();
    assert_eq!(buttons.len(), 1);
    buttons[0].invoke_accessible_default_action();

    // Verify defaults restored (test invariant, not specific values)
    // Longitude resets to some value in [-180, 180]
    assert!(app.get_camera_longitude() >= -180.0 && app.get_camera_longitude() <= 180.0);
    // Zoom resets to a reasonable default (not the extreme we set)
    assert_ne!(app.get_camera_zoom(), 0.9);
}
```

Note: each test function that calls `init_no_event_loop()` initializes a fresh backend. However,
`init_no_event_loop()` uses `set_platform()` which panics if called twice in the same process. Each
integration test file (`tests/slint_ui.rs`) must therefore use a `std::sync::OnceLock` or similar
to call `init_no_event_loop()` exactly once per process:

```rust
// At the top of tests/slint_ui.rs
fn init() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INIT.get_or_init(|| {
        i_slint_backend_testing::init_no_event_loop();
    });
}

#[test]
fn test_something() {
    init();
    let app = MainWindow::new().unwrap();
    // ...
}
```

Unlike `init_integration_test_with_*` (which requires one test per process), `init_no_event_loop()`
sets up the testing backend in a mode where multiple `MainWindow::new()` instances can be created
in the same process. The Slint backend is not re-initialized on each `MainWindow::new()`.

### Pattern B: Async test with event loop (click simulation)

Use `init_integration_test_with_system_time()` when you need `await` for click simulation. This
initializes the Slint event loop and can only be called once per process — meaning the entire test
binary can contain only **one** test function using this mode. Place async event loop tests in a
dedicated file if needed.

```rust
// In tests/slint_ui_clicks.rs — one test function only
#[test]
fn test_preset_button_click() {
    i_slint_backend_testing::init_integration_test_with_system_time();

    slint::spawn_local(async move {
        let app = MainWindow::new().unwrap();

        // Register callback to capture whether preset was applied
        let preset_applied = std::rc::Rc::new(std::cell::RefCell::new(false));
        app.on_apply_preset({
            let flag = preset_applied.clone();
            move |_idx| { *flag.borrow_mut() = true; }
        });

        // Find a preset button by text
        let buttons: Vec<_> =
            ElementHandle::find_by_accessible_label(&app, "Europe").collect();
        assert_eq!(buttons.len(), 1);

        // Simulate a real click (move + press + idle + release)
        buttons[0].single_click(PointerEventButton::Left).await;

        assert!(*preset_applied.borrow());

        slint::quit_event_loop().unwrap();
    })
    .unwrap();

    slint::run_event_loop().unwrap();
}
```

### Pattern C: Mock time for animations and timers

```rust
#[test]
fn test_timer_triggers_redraw() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INIT.get_or_init(|| {
        i_slint_backend_testing::init_integration_test_with_mock_time();
    });

    slint::spawn_local(async move {
        let app = MainWindow::new().unwrap();
        // advance mock time by 2 minutes to trigger the periodic sun timer
        i_slint_backend_testing::mock_elapsed_time(
            std::time::Duration::from_secs(120)
        );
        // observe side effects...
        slint::quit_event_loop().unwrap();
    }).unwrap();

    slint::run_event_loop().unwrap();
}
```

### Pattern D: Config round-trip test

```rust
#[test]
fn test_apply_read_config_roundtrip() {
    init(); // OnceLock-guarded init_no_event_loop()

    let app = MainWindow::new().unwrap();

    // apply_config_to_window writes to the Slint component
    let cfg = AppConfig {
        camera: CameraParams {
            longitude: 13.4,
            latitude: 52.5,
            zoom: 0.5,
            ..Default::default()
        },
        ..Default::default()
    };
    apply_config_to_window(&app, &cfg);

    // read_config_from_window reads back from the component
    let roundtrip = read_config_from_window(&app);

    assert_relative_eq!(roundtrip.camera.longitude, cfg.camera.longitude, epsilon = 1e-4);
    assert_relative_eq!(roundtrip.camera.latitude, cfg.camera.latitude, epsilon = 1e-4);
}
```

### Pattern E: Conditional visibility test

```rust
#[test]
fn test_datetime_controls_visibility() {
    init();
    let app = MainWindow::new().unwrap();

    // By default, custom datetime is off
    assert!(!app.get_use_custom_datetime());

    // Hour/day sliders should not be visible in this state;
    // find them and check geometry — elements hidden by `if` have size 0
    // (or are not in the tree at all)
    let hour_sliders: Vec<_> =
        ElementHandle::find_by_element_id(&app, "MainWindow::hour-slider").collect();
    // Either absent from the tree or zero size
    if let Some(slider) = hour_sliders.first() {
        assert_eq!(slider.size().width, 0.0);
    }

    // Enable custom datetime
    app.set_use_custom_datetime(true);

    // Now the slider should appear
    let hour_sliders: Vec<_> =
        ElementHandle::find_by_element_id(&app, "MainWindow::hour-slider").collect();
    assert!(!hour_sliders.is_empty());
}
```

## 5. Version Compatibility

### Published versions

```
i-slint-backend-testing versions:
1.15.1  — 2026-02-12
1.15.0  — 2026-02-04
1.14.1  — 2025-10-23
1.14.0  — 2025-10-21
1.13.1  — 2025-09-11
1.13.0  — 2025-09-03
1.12.1  — 2025-06-25
1.12.0  — 2025-06-16
1.11.0  — 2025-04-23
1.10.0  — 2025-02-28
1.9.2   — 2025-01-13
...
```

Every Slint release publishes a matching `i-slint-backend-testing` at the same version number.
The version must be pinned exactly with `=x.y.z` syntax.

### Cargo.toml for this project

Since the project uses `slint = { version = "~1.15", ... }`, the matching dev-dependency is:

```toml
[dev-dependencies]
i-slint-backend-testing = "=1.15.1"
```

If/when the project upgrades to Slint 1.15.1 (it currently accepts both 1.15.0 and 1.15.1 via
`~1.15`), pin to whichever resolved version `Cargo.lock` selects:

```bash
cargo metadata --format-version 1 | python3 -c "
import json,sys
data=json.load(sys.stdin)
for p in data['packages']:
    if p['name']=='slint': print(p['version'])
"
```

Then use that exact version in `i-slint-backend-testing = "=<resolved-version>"`.

### Notable API changes between versions

The `i-slint-backend-testing` crate explicitly does **not** follow semver and may break in any
patch release. Key changes found across recent versions:

- **1.9.x and earlier**: `visit_descendants` existed; `query_descendants` and `ElementQuery`
  builder were not yet available (added in PR #5433, which landed around 1.7–1.9 timeframe).
- **1.10+**: `ElementQuery::from_root()` added (previously required the `ElementRoot` trait
  directly).
- **1.15**: `accessible_id()` method added to `ElementHandle` (reads the `accessible-id` property
  added to the Slint language in 1.15); `accessible_expanded/expandable/read_only` methods added.
- The `scroll()` method has **never** been present on `ElementHandle` through 1.15.1.
- Keyboard simulation has **never** been part of `ElementHandle` — use `dispatch_event()` instead.

There are no known breaking changes to the `init_*` functions or the core `find_by_element_id` /
`find_by_accessible_label` / `invoke_accessible_default_action` methods across any recent version.

## 6. End-to-End Testing Patterns

### In-process integration test (recommended)

The Slint team's documentation does not describe or recommend launching the application as a
subprocess. The intended pattern is **in-process integration testing**:

1. The application logic lives in a library crate (`lib.rs` exports functions like
   `apply_config_to_window`, `read_config_from_window`).
2. Tests in `tests/slint_ui.rs` call `include_modules!()` (or use `use sunlit_earth::MainWindow`)
   to get the Slint component, initialize the testing backend, and exercise the UI logic.
3. The test binary links the same code as the application but never calls `main()`.

This approach is sufficient for all UI logic testing needs: callback wiring, property round-trips,
conditional visibility, default values, and config persistence.

### Subprocess testing (not recommended for this project)

There is no Slint-specific subprocess test harness. The general approach would be:

1. Build the binary (`CARGO_BIN_EXE_sunlit_earth`).
2. Launch it with a headless flag and an IPC channel.
3. Send commands and observe effects.

This is not viable for Sunlit Earth because: the app has no headless mode; the event loop is
blocking; rendering is wgpu-based (not in Slint's renderer); and there is no IPC mechanism. The
cost of adding all this infrastructure far exceeds the benefit given that the renderer is already
testable directly via `tests/render_pipeline.rs`.

## 7. Limitations and Workarounds

### No pixel output

The testing backend renders no pixels. Text is measured with a fixed font size (not system fonts).
Element geometry (`size()`, `absolute_position()`) is calculated from Slint's layout engine and
is available, but visual appearance cannot be verified. This is by design — visual regression
testing uses the wgpu offscreen render pipeline in `tests/render_pipeline.rs`, not the Slint
testing backend.

### No scroll simulation via ElementHandle

`ElementHandle` has no `scroll()` method. Workaround: `window().dispatch_event(WindowEvent::PointerScrolled { ... })`.

### No keyboard input via ElementHandle

`ElementHandle` has no keyboard methods. Workaround: `window().dispatch_event(WindowEvent::KeyPressed { text: ... })`.

### One-init-per-process for event-loop modes

`init_integration_test_with_system_time()` and `init_integration_test_with_mock_time()` can only
be called once per process (they call `set_platform()` which panics on re-initialization). Tests
using these must be isolated in a separate test binary with a single `#[test]` function, or use
a `OnceLock` guard. Cargo runs each `tests/*.rs` file as a separate test binary, so a file with
multiple `#[test]` functions that all use the async/event-loop pattern will panic on the second
test.

`init_no_event_loop()` has the same restriction. Use the `OnceLock` guard pattern shown in
Pattern A to share the initialization across multiple test functions within one file.

### Slint testing backend and wgpu cannot coexist in the same process

The testing backend calls `set_platform()`, which replaces Slint's default backend (winit + wgpu).
Once the testing backend is set, Slint does not use wgpu at all. This means tests that use
`i-slint-backend-testing` cannot also exercise the wgpu rendering pipeline through Slint's normal
rendering notifier.

The workaround is **separate test binaries**:

- `tests/slint_ui.rs` — uses `i-slint-backend-testing`, no wgpu
- `tests/render_pipeline.rs` — creates its own wgpu context, no Slint testing backend
- `tests/shading.rs` — compute shader tests, no Slint

This separation is already implicit in the current project structure since GPU tests don't use
the Slint testing backend.

### `elements inside `if`` clauses may not appear in the tree

Elements under `if condition:` are not in the element tree when the condition is false. Querying
them returns an empty iterator. Test visibility by asserting the iterator is empty when the
condition is false, and non-empty when true. Do not assume `size() == 0` is a reliable indicator
of hidden-via-if elements (vs. genuinely zero-sized elements).

### `accessible-value` write requires explicit declaration

`set_accessible_value()` only works if the `.slint` code explicitly declares
`accessible-value: ...` as a writable expression. For standard Slint `Slider` widgets, the
`accessible-value` is read-only (bound to the slider's `value`). To set a slider value in tests,
use `app.set_camera_longitude(...)` or `app.set_<property>(...)` directly instead.

## 8. `accessible-role` and `accessible-label` in .slint

### Automatic accessible properties on built-in widgets

Slint's standard library widgets set accessible properties automatically:

| Element | Automatic `accessible-role` | Automatic `accessible-label` source |
| --- | --- | --- |
| `Button` (std-widgets) | `button` | `text` property |
| `CheckBox` (std-widgets) | `checkbox` | `text` property |
| `Slider` (std-widgets) | `slider` | (none by default) |
| `ComboBox` (std-widgets) | `combobox` | (none by default) |
| `Text` (builtin) | `text` | `text` property |
| `TextInput` (builtin) | `text-input` | (none by default) |
| `Image` (builtin) | `image` | `accessible-label` if set |
| Custom elements | `none` | (none) |

For `Button` and `CheckBox`, `find_by_accessible_label("Submit")` works automatically because the
widget sets `accessible-label: self.text`. For sliders, comboboxes, and custom elements, you must
either:

1. Set `accessible-label` explicitly in `.slint`:
   ```slint
   longitude-slider := Slider {
       accessible-label: "Longitude";
       ...
   }
   ```
2. Or use `find_by_element_id("MainWindow::longitude-slider")` instead.

### The `accessible-id` property (added in Slint 1.15)

Slint 1.15 added `accessible-id` as a new property, readable via `ElementHandle::accessible_id()`.
This is distinct from the element's structural identifier used by `find_by_element_id`. It is
intended as a stable test automation ID that can be set explicitly:

```slint
longitude-slider := Slider {
    accessible-id: "longitude-slider";
    ...
}
```

Unlike the structural `id()`, `accessible-id` is independent of component nesting and can be
used to uniquely identify elements in dynamic lists:

```slint
for i in [0, 1, 2, 3, 4]: Rectangle {
    accessible-role: button;
    accessible-id: "preset-btn-" + i;
}
```

To use `accessible-id` for element lookup, use `match_predicate`:

```rust
let btn = ElementQuery::from_root(&app)
    .match_predicate(|el| el.accessible_id().as_deref() == Some("preset-btn-0"))
    .find_first();
```

Note: there is no `find_by_accessible_id` convenience function in 1.15.1. Use `match_predicate`.

### All `AccessibleRole` enum values (Slint 1.15)

`none`, `button`, `checkbox`, `combobox`, `groupbox`, `image`, `list`, `slider`, `spinbox`,
`tab`, `tab-list`, `tab-panel`, `text`, `table`, `tree`, `progress-indicator`, `text-input`,
`switch`, `list-item`, `radio-button`

The `accessible-role` must be set to something other than `none` for any other `accessible-*`
property to take effect. Custom elements that should be testable need an explicit `accessible-role`
set to the most appropriate value.

### Making Sunlit Earth elements testable

The current `ui/main.slint` uses Slint standard widgets (`Button`, `CheckBox`, `Slider`, `ComboBox`)
which all receive automatic `accessible-role`. Buttons are findable by text via
`find_by_accessible_label`. Sliders, checkboxes, and comboboxes are findable by element ID via
`find_by_element_id("MainWindow::longitude-slider")` etc.

No `.slint` changes are required to make the existing UI elements testable. However, adding
`accessible-label` to sliders would improve the ergonomics of test code:

```slint
longitude-slider := Slider {
    accessible-label: "Longitude";
    ...
}
```

Then tests can write:

```rust
ElementHandle::find_by_accessible_label(&app, "Longitude").next().unwrap()
```

## Sources and Confidence

| Finding | Source | Confidence |
| --- | --- | --- |
| Verbatim README.md text with code examples | [github.com/slint-ui/slint internal/backends/testing/README.md](https://raw.githubusercontent.com/slint-ui/slint/master/internal/backends/testing/README.md) (raw) | High |
| Full ElementHandle method list v1.15.1 | [docs.rs/i-slint-backend-testing/1.9.2/struct.ElementHandle](https://docs.rs/i-slint-backend-testing/1.9.2/i_slint_backend_testing/struct.ElementHandle.html) + v1.15.1 | High |
| ElementQuery builder methods | [PR #5433](https://github.com/slint-ui/slint/pull/5433), docs.rs | High |
| Qualified ID format `"ComponentName::element-name"` | Official README example `"App::ta"` | High |
| Automatic accessible-role on std widgets | [docs.slint.dev common properties](https://docs.slint.dev/latest/docs/slint/reference/common/) | High |
| `accessible-id` added in 1.15 | [crates.io version history](https://crates.io/api/v1/crates/i-slint-backend-testing/versions), accessible_id() present in 1.15.1 docs | High |
| Version list and release dates | [crates.io API](https://crates.io/api/v1/crates/i-slint-backend-testing/versions) | High |
| `dispatch_event(WindowEvent::KeyPressed)` for keyboard | [docs.slint.dev WindowEvent](https://docs.slint.dev/latest/docs/rust/slint/platform/enum.WindowEvent), virtual keyboard example | High |
| One-init-per-process constraint explicit in source | [lib.rs source](https://github.com/slint-ui/slint/blob/master/internal/backends/testing/lib.rs) doc comments | High |
| No scroll/keyboard on ElementHandle | Exhaustive method list from docs.rs 1.9.2 and 1.15.1, confirmed no such methods | High |
| for-loop element ID limitation | [Issue #4891](https://github.com/slint-ui/slint/issues/4891) | High |
| `if`-guarded elements absent from tree | Slint design (conditional rendering removes element from tree entirely) | Medium |
| `accessible-value` write requires explicit declaration | docs.rs `set_accessible_value` note | Medium |

## Open Questions Resolved vs. Still Open

**Resolved by this research:**

1. **Element ID format**: Must be `"ComponentName::element-name"` with the export component name
   as prefix. Just `"element-name"` does not work.
2. **Property read/write in tests**: Use generated `get_*()/set_*()` methods on the component
   handle, not `ElementHandle`. `ElementHandle` only exposes accessible properties.
3. **Keyboard and scroll simulation**: Not available via `ElementHandle`. Use
   `window().dispatch_event(WindowEvent::...)` directly.
4. **Version to pin**: `"=1.15.1"` (or `"=1.15.0"` if the lock file resolves to that).
5. **Slint+wgpu coexistence**: They cannot coexist in the same process. Keep `tests/slint_ui.rs`
   separate from GPU integration tests (already the case).
6. **Multiple tests with `init_no_event_loop`**: Use a `OnceLock` guard; the backend is
   initialized once per process, but multiple `MainWindow::new()` can be created.

**Still open:**

1. **`if`-hidden element geometry**: Whether `size()` returns `{0,0}` or the element is simply
   absent from the tree for `if`-guarded elements needs empirical verification when writing the
   actual tests.
2. **Button text in preset grid**: The preset buttons in `ui/main.slint` use text like "Europe",
   "N. America" etc. These should be findable via `find_by_accessible_label`. This needs
   confirmation that `Button.text` is indeed surfaced as `accessible-label` in the testing backend
   (expected yes per documented behavior).
3. **`slint::include_modules!()` in test binary context**: Whether `include_modules!()` must be
   called in the test binary or whether re-exporting `MainWindow` from the library crate is
   sufficient for `ElementRoot` to be implemented. The standard approach is to have the test file
   include the generated types directly.
