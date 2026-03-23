---
title: Mouse Controls Overhaul
status: implementing
stakes: medium
created: 2026-03-23
---

# Mouse Controls Overhaul — Implementation Plan

## Summary

Overhaul mouse controls in the viewport to support button-specific drag behaviors: left drag for globe rotation (already works but needs button restriction), right drag for framing (offset X/Y) with inverted/"scrolling" direction, middle drag for pitch/yaw, and left+right simultaneous drag for tilt. Invert the framing slider direction so positive slider values move the frame opposite to the current behavior. Scroll wheel zoom remains unchanged. The changes span three files: Slint UI (button state tracking and callback routing), Rust main.rs (new drag callbacks), and camera.rs (offset sign inversion). No shader, uniform layout, or renderer pipeline changes are required.

## Stakes Classification

**Level**: Medium
**Rationale**: The changes touch the UI interaction layer (Slint TouchArea), the callback wiring (main.rs), and the camera matrix (camera.rs), but do not affect the GPU pipeline, shaders, or uniform layout. The primary complexity is in the Slint button-state tracking pattern. The offset sign inversion in camera.rs affects exactly one line but changes the visual behavior of existing slider values — saved configs will appear inverted until the user re-adjusts. Rollback is straightforward: revert three files.

## Context

**Research**: [`docs/plans/2026-03-23-mouse-controls-research.md`](2026-03-23-mouse-controls-research.md)
**Affected Areas**: `ui/main.slint`, `src/main.rs`, `src/scene/camera.rs`

## Success Criteria

- [ ] Left mouse drag rotates the globe (longitude/latitude) — unchanged behavior, but no longer triggered by right or middle drag
- [ ] Right mouse drag adjusts framing (offset X/Y) with inverted/"scrolling" direction (drag right moves frame left)
- [ ] Middle mouse drag adjusts pitch (vertical) and yaw (horizontal)
- [ ] Left + right simultaneous drag adjusts tilt (horizontal movement only)
- [ ] Scroll wheel adjusts zoom — unchanged behavior
- [ ] Framing sliders are inverted: positive slider value moves the frame in the opposite direction compared to the previous behavior
- [ ] Right-click context menu is suppressed on the viewport
- [ ] Existing camera tests updated for the offset sign change
- [ ] `cargo build` succeeds with no warnings
- [ ] `cargo clippy` passes
- [ ] `cargo test` passes

## Implementation Steps

### Phase 1: Invert Framing Offset in Camera

Invert the sign of the offset in `mvp_matrix()` so that positive offset slider values move the frame in the opposite direction. This is a prerequisite for Phase 3 (right-drag framing) so that the drag direction math works out correctly.

#### Step 1.1: Negate offset in `mvp_matrix()`

- **File**: `src/scene/camera.rs` (line 141)
- **Action**: Change the offset translation from `Vec3::new(self.offset_x, self.offset_y, 0.0)` to `Vec3::new(-self.offset_x, -self.offset_y, 0.0)`.

#### Step 1.2: Update the camera offset test

- **File**: `src/scene/camera.rs` (test module, `mvp_with_positive_x_offset_shifts_right`)
- **Action**: Rename to `mvp_with_positive_x_offset_shifts_left` and invert the assertion from `clip_yes.x > clip_no.x` to `clip_yes.x < clip_no.x`.
- **Verification**: `cargo test camera` passes.

### Phase 2: Add Button State Tracking and Multiple Callbacks in Slint

Replace the single `mouse-drag` callback with four button-specific callbacks and add button-state tracking properties to the TouchArea.

#### Step 2.1: Add new callback declarations

- **File**: `ui/main.slint` (around line 82-84)
- **Action**: Replace `callback mouse-drag(float, float);` with:

```slint
callback mouse-drag-globe(float, float);
callback mouse-drag-frame(float, float);
callback mouse-drag-orient(float, float);
callback mouse-drag-tilt(float, float);
```

Keep `callback mouse-scroll(float);` unchanged.

#### Step 2.2: Add button-state tracking properties

- **File**: `ui/main.slint` (inside `image-container`, near existing `last-mouse-x` / `last-mouse-y`)
- **Action**: Add three boolean properties:

```slint
property <bool> left-held: false;
property <bool> right-held: false;
property <bool> middle-held: false;
```

#### Step 2.3: Update `pointer-event` handler for button tracking

- **File**: `ui/main.slint` (TouchArea `pointer-event` handler)
- **Action**: Track button state and record mouse position on both down and up events (prevents position jumps when transitioning between drag modes):

```slint
pointer-event(event) => {
    if event.button == PointerEventButton.left {
        image-container.left-held = event.kind == PointerEventKind.down;
    }
    if event.button == PointerEventButton.right {
        image-container.right-held = event.kind == PointerEventKind.down;
    }
    if event.button == PointerEventButton.middle {
        image-container.middle-held = event.kind == PointerEventKind.down;
    }
    if event.kind == PointerEventKind.down || event.kind == PointerEventKind.up {
        image-container.last-mouse-x = self.mouse-x;
        image-container.last-mouse-y = self.mouse-y;
    }
}
```

#### Step 2.4: Update `moved` handler with button-aware routing

- **File**: `ui/main.slint` (TouchArea `moved` handler)
- **Action**: Priority-ordered routing (left+right must be checked before individual buttons):

```slint
moved => {
    if image-container.left-held && image-container.right-held {
        root.mouse-drag-tilt(
            (self.mouse-x - image-container.last-mouse-x) / 1px,
            (self.mouse-y - image-container.last-mouse-y) / 1px
        );
    } else if image-container.left-held {
        root.mouse-drag-globe(
            (self.mouse-x - image-container.last-mouse-x) / 1px,
            (self.mouse-y - image-container.last-mouse-y) / 1px
        );
    } else if image-container.right-held {
        root.mouse-drag-frame(
            (self.mouse-x - image-container.last-mouse-x) / 1px,
            (self.mouse-y - image-container.last-mouse-y) / 1px
        );
    } else if image-container.middle-held {
        root.mouse-drag-orient(
            (self.mouse-x - image-container.last-mouse-x) / 1px,
            (self.mouse-y - image-container.last-mouse-y) / 1px
        );
    }
    image-container.last-mouse-x = self.mouse-x;
    image-container.last-mouse-y = self.mouse-y;
}
```

#### Step 2.5: Context menu suppression

The TouchArea should suppress the native right-click context menu by consuming the pointer events. Verify empirically. If it doesn't, fall back to platform-specific suppression (follow-up task).

### Phase 3: Wire Up Rust Callbacks

Replace the single `on_mouse_drag` callback in `main.rs` with four button-specific callbacks.

#### Step 3.1: Rename `on_mouse_drag` to `on_mouse_drag_globe`

- **File**: `src/main.rs` (lines 235-267)
- **Action**: Rename only. The implementation (tilt-corrected rotation, zoom-based sensitivity) is identical.

#### Step 3.2: Add `on_mouse_drag_frame` callback

- **File**: `src/main.rs` (after the globe drag callback)
- **Action**: Right-drag framing. After the Phase 1 offset inversion, positive dx → increase offset_x → frame moves left (correct "scrolling" behavior). Positive dy → increase offset_y → frame moves down (also correct).

```rust
window.on_mouse_drag_frame(move |dx, dy| {
    let Some(win) = window_weak.upgrade() else { return; };
    let zoom = win.get_camera_zoom();
    let sensitivity = 0.002 * zoom_to_distance(zoom) / 8.0;

    let new_x = (win.get_camera_offset_x() + dx * sensitivity).clamp(-3.0, 3.0);
    let new_y = (win.get_camera_offset_y() + dy * sensitivity).clamp(-3.0, 3.0);

    win.set_camera_offset_x(new_x);
    win.set_camera_offset_y(new_y);
    win.window().request_redraw();
    config_timer_handle.restart();
});
```

#### Step 3.3: Add `on_mouse_drag_orient` callback

- **File**: `src/main.rs` (after the frame drag callback)
- **Action**: Middle-drag pitch/yaw. Horizontal dx → yaw, vertical dy → pitch (negated because screen Y is downward).

```rust
window.on_mouse_drag_orient(move |dx, dy| {
    let Some(win) = window_weak.upgrade() else { return; };
    let degrees_per_px = 0.2;

    let new_yaw = (win.get_camera_yaw() + dx * degrees_per_px).clamp(-90.0, 90.0);
    let new_pitch = (win.get_camera_pitch() - dy * degrees_per_px).clamp(-90.0, 90.0);

    win.set_camera_yaw(new_yaw);
    win.set_camera_pitch(new_pitch);
    win.window().request_redraw();
    config_timer_handle.restart();
});
```

#### Step 3.4: Add `on_mouse_drag_tilt` callback

- **File**: `src/main.rs` (after the orient drag callback)
- **Action**: Left+right simultaneous drag. Only horizontal dx controls tilt; dy is ignored.

```rust
window.on_mouse_drag_tilt(move |dx, _dy| {
    let Some(win) = window_weak.upgrade() else { return; };
    let degrees_per_px = 0.5;

    let new_tilt = win.get_camera_tilt() + dx * degrees_per_px;
    let wrapped_tilt = ((new_tilt + 180.0) % 360.0 + 360.0) % 360.0 - 180.0;

    win.set_camera_tilt(wrapped_tilt);
    win.window().request_redraw();
    config_timer_handle.restart();
});
```

#### Step 3.5: Remove the old `on_mouse_drag` callback

- **Verification**: `cargo build` succeeds. All four drag modes work.

### Phase 4: Edge Cases and Polish

#### Step 4.1: Button transition testing

Manual testing:
1. Start left-dragging, then press right → should switch to tilt mode cleanly
2. Release one button while both held → should transition to single-button mode
3. Press and release right button without moving → no accidental drag

#### Step 4.2: Context menu verification

Right-click viewport → no context menu should appear.

#### Step 4.3: Sensitivity tuning

Starting constants (tune empirically):
- Globe rotation: `0.3 * zoom_to_distance(zoom) / 8.0` (existing)
- Framing: `0.002 * zoom_to_distance(zoom) / 8.0`
- Pitch/Yaw: `0.2` deg/px
- Tilt: `0.5` deg/px

### Phase 5: Final Verification

- `cargo test` — all tests pass
- `cargo clippy` — no warnings
- Full manual integration test of all controls and sliders

## Known Limitations

1. **Config migration**: Existing saved configs will have inverted framing offsets. Users must re-adjust or reset to defaults. Acceptable for early-stage project.
2. **Context menu**: TouchArea should suppress it by consuming events, but needs empirical verification on Windows.
3. **Sensitivity**: Starting-point constants may need adjustment after testing.
