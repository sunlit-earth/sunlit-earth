# Mouse Controls Overhaul — Research

## Goal

Overhaul mouse controls so each button controls a different camera parameter:

| Button Combo | Action |
|---|---|
| Left drag | Latitude / Longitude (globe rotation) — already works this way |
| Scroll wheel | Zoom — already works this way |
| Right drag | Framing (offset X/Y), with inverted direction |
| Middle drag | Pitch / Yaw |
| Left + Right drag | Tilt (horizontal movement only) |

Additional: Invert framing slider direction (positive slider = frame moves opposite direction from current).

## Current Implementation

### Mouse Handling (main.rs:235-282)

Two callbacks, no button discrimination:

- **`on_mouse_drag(dx, dy)`** — Rotates globe (lat/lon). Compensates for tilt by rotating the delta vector. Sensitivity scales with zoom distance.
- **`on_mouse_scroll(delta)`** — Adjusts zoom. Sensitivity: 0.0003 per scroll unit.

### TouchArea (ui/main.slint:971-995)

Single `viewport-touch` TouchArea covering the viewport:

- `pointer-event(event)` — Records mouse position on `down`
- `moved` — Computes delta, fires `root.mouse-drag(dx, dy)`
- `scroll-event` — Fires `root.mouse-scroll(delta_y)`

Fires `mouse-drag` on ANY button press — no button differentiation.

### Camera Parameters

| Parameter | Range | Mouse Control | Slider |
|---|---|---|---|
| Longitude | [-180, 180]° | Drag X | Yes |
| Latitude | [-89, 89]° | Drag Y | Yes |
| Zoom | [0.0, 1.0] | Scroll | Yes |
| Offset X | [-3.0, 3.0] | None | Yes |
| Offset Y | [-3.0, 3.0] | None | Yes |
| Tilt | [-180, 180]° | None | Yes |
| Yaw | [-90, 90]° | None | Yes |
| Pitch | [-90, 90]° | None | Yes |

### Framing Implementation (scene/camera.rs:139-143)

Offset is a post-projection clip-space translation:

```rust
let offset = Mat4::from_translation(Vec3::new(self.offset_x, self.offset_y, 0.0));
offset * base_mvp
```

- Positive `offset_x` → image shifts **right**
- Positive `offset_y` → image shifts **up**
- Slider range: ±3.0 with snap-to-zero at ±0.05

## Slint PointerEvent API

### PointerEvent struct

| Field | Type | Description |
|---|---|---|
| `button` | `PointerEventButton` | Which button changed state |
| `kind` | `PointerEventKind` | down, up, move, cancel |
| `modifiers` | `KeyboardModifiers` | Alt/Ctrl/Shift/Meta |

### PointerEventButton enum

| Variant | Value |
|---|---|
| `other` | 0 |
| `left` | 1 |
| `right` | 2 |
| `middle` | 3 |

### Key Constraints

1. **`moved()` has no button info** — only fires during drag, no parameters
2. **`pointer-event` fires on move too** — `PointerEventKind.move` carries position but `button` is `other` during movement
3. **No native multi-button tracking** — must manually track held buttons via state flags
4. **`pressed` property is a single bool** — no button identity

### Recommended Pattern

Track button state with boolean properties in Slint:

```slint
property <bool> left-held: false;
property <bool> right-held: false;
property <bool> middle-held: false;

pointer-event(e) => {
    if (e.button == PointerEventButton.left) {
        left-held = e.kind == PointerEventKind.down;
    }
    if (e.button == PointerEventButton.right) {
        right-held = e.kind == PointerEventKind.down;
    }
    if (e.button == PointerEventButton.middle) {
        middle-held = e.kind == PointerEventKind.down;
    }
}
```

Then in `moved`, check which buttons are held and route to different Rust callbacks.

## Architecture Decision

Two approaches for routing drag events:

**Option A: Multiple Rust callbacks** — Slint checks button state and calls different callbacks (`mouse-drag-globe`, `mouse-drag-frame`, `mouse-drag-orient`, `mouse-drag-tilt`). Pros: Rust handlers stay simple. Cons: More callbacks to wire up.

**Option B: Single callback with button flags** — Slint passes button state as parameters: `mouse-drag(dx, dy, left, right, middle)`. Rust side routes internally. Pros: One callback. Cons: Wider parameter list.

**Recommendation**: Option A (multiple callbacks) — cleaner separation of concerns, each handler does one thing.

## Framing Slider Inversion

Current: positive slider → positive offset → image moves right/up.
Desired: positive slider → image moves left/down (opposite).

Two approaches:
1. **Negate in camera.rs** — `offset = Mat4::from_translation(Vec3::new(-self.offset_x, -self.offset_y, 0.0))`. Simple, one change.
2. **Negate in UI** — Bind slider value with negation. More complex.

**Recommendation**: Negate in camera.rs `mvp_matrix()` — single point of change.

## Right-Drag Framing Direction

User requirement: "dragging the mouse to the right should move the frame to the left."

This means: positive dx → decrease offset_x (or if sliders are already inverted, positive dx → increase offset_x which displays as leftward movement). Need to think about this carefully with the slider inversion.

After slider inversion (positive offset_x → image moves left):
- Right drag → positive dx → we want frame to move left → increase offset_x → correct, natural mapping

So: apply `dx` positively to `offset_x` after slider inversion. The sign will be natural.

For Y-axis: dragging down (positive dy) should move frame up → increase offset_y. After inversion (positive offset_y → image moves down), we need: positive dy → decrease offset_y. So negate dy for offset_y.

Actually let me reconsider. The user said "dragging the mouse to the right should move the frame to the left." This is like scrolling/panning behavior — you drag the "canvas" and the viewport moves opposite. Let me just make sure the implementation gets this right during planning.

## Tilt via Left+Right

Track both left and right held simultaneously. When both are pressed, horizontal mouse movement controls tilt. This requires:
1. Detecting the combo (both flags true)
2. Prioritizing it over individual button actions (left-drag for lat/lon, right-drag for framing)
3. Starting tilt control only when the second button is pressed while the first is already held

Edge case: if user is dragging with left (rotating globe) and then presses right, we should switch to tilt mode and stop rotation.
