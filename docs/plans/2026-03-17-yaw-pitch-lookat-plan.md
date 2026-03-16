# Plan: Yaw/Pitch Look-At Redesign (2026-03-17)

## Summary

Replace the post-view rotation implementation of yaw and pitch in `OrbitalCamera::view_matrix()` with a look-at target offset approach. The current implementation applies `Rx(pitch) * Ry(yaw)` as post-view rotations, which at the narrow 20-degree FOV produces a visual result nearly identical to the screen-space offset sliders. The redesign shifts the camera's look-at target instead: `look_at_rh(eye, origin + offset, up)` where the offset is derived from yaw/pitch via `sin()`, making these controls shift the camera's gaze toward the Earth's surface rather than rotating the view in place. This is a single-method change in `camera.rs` with updated unit tests. No other files are affected.

## Stakes Classification

**Level**: Low
**Rationale**: The change is confined to a single method (`view_matrix()`) in a single file (`camera.rs`). The `CameraParams` struct, `FrameState`, UI sliders, renderer pipeline, shaders, and wallpaper export are all untouched. The existing test suite covers the surrounding infrastructure. Rollback is a one-commit revert.

## Context

**Research**: [`docs/plans/2026-03-16-camera-controls-research.md`](2026-03-16-camera-controls-research.md) (original camera controls research)
**Prior Plan**: [`docs/plans/2026-03-16-camera-controls-plan.md`](2026-03-16-camera-controls-plan.md) (implemented the current post-view rotation approach)
**Affected Areas**: `src/scene/camera.rs` only

## Success Criteria

- [x] `view_matrix()` uses look-at target offset instead of post-view yaw/pitch rotations
- [x] At yaw=0, pitch=0: identical matrix to the no-yaw/pitch case
- [x] At pitch=90: look-at point shifts by 1.0 along the camera's local up axis (Earth's radius)
- [x] `sin()` mapping provides natural scaling: linear near zero, saturating near 90 degrees
- [x] Tilt (roll) remains unchanged as a post-view Z rotation
- [x] All existing tilt tests pass unchanged
- [x] All existing offset tests pass unchanged
- [x] New yaw/pitch tests pass, replacing the old rotation-based tests
- [x] `cargo test camera` passes
- [x] `cargo clippy` passes

## Implementation Steps

### Phase 1: Update `view_matrix()` and Tests

#### Step 1.1: Write new yaw/pitch unit tests (RED)

- **Files**: `src/scene/camera.rs` (test module, lines 136-487)
- **Action**: Replace the existing yaw/pitch tests with new tests that verify the look-at target offset behavior. Remove these existing tests that assert post-view rotation behavior:
  - `view_with_yaw_rotates_horizontally` (line 277)
  - `view_with_negative_yaw_mirrors_positive` (line 295)
  - `view_with_yaw_preserves_determinant` (line 317)
  - `view_with_pitch_rotates_vertically` (line 327)
  - `view_with_negative_pitch_mirrors_positive` (line 344)
  - `view_with_pitch_preserves_determinant` (line 365)
  - `rotations_compose_correctly` (line 389)

  Add the following new tests:

  - **`view_with_zero_yaw_pitch_unchanged`**: Construct two cameras at (0, 0, 5.0). Set yaw=0, pitch=0 on one. Assert `view_matrix()` produces the same result as the default camera (no yaw/pitch). Uses `assert_relative_eq!` on the column arrays.

  - **`view_with_pitch_shifts_look_target_up`**: Construct a camera at (0, 0, 5.0) with pitch=30. Transform a point at (0, 0.5, 0) (above the origin) through both the pitched and un-pitched view matrices. Assert the Y component in view space is closer to zero (more centered) with the pitch applied, because the camera is now looking upward toward that point.

  - **`view_with_yaw_shifts_look_target_right`**: Construct a camera at (0, 0, 5.0) with yaw=30. Transform a point at (0.5, 0, 0) (to the right of the origin) through both view matrices. Assert the X component in view space is closer to zero (more centered) with the yaw applied, because the camera is now looking rightward toward that point.

  - **`pitch_90_looks_at_surface`**: Construct a camera at (0, 0, 5.0) with pitch=90. The look-at target should be offset by 1.0 along the camera's local up axis. Transform the origin through the pitched view matrix and the offset point (0, 1, 0) through the un-pitched view matrix. Assert that the pitched camera's forward direction has a significant upward component by checking that the view matrix transforms the origin to a different view-space Y than the un-pitched case.

  - **`view_with_negative_yaw_mirrors_positive`**: Construct cameras with yaw=30 and yaw=-30. Transform a point at (1, 0, 0) through each view matrix and the base (yaw=0) matrix. Assert the X deltas from the base have opposite signs.

  - **`view_with_negative_pitch_mirrors_positive`**: Construct cameras with pitch=30 and pitch=-30. Transform a point at (0, 1, 0) through each view matrix and the base (pitch=0) matrix. Assert the Y deltas from the base have opposite signs.

  - **`yaw_pitch_preserves_determinant`**: For yaw values in [-90, -45, 0, 45, 90] and pitch values in [-90, -45, 0, 45, 90], assert the view matrix determinant is non-zero (absolute value > 0.5).

  Keep these existing tests unchanged:
  - `view_with_zero_rotations_unchanged` (covers zero case for all three rotations)
  - `all_rotations_zero_equals_no_rotation` (same, different camera position)
  - All tilt tests (`view_with_180_tilt_flips_vertical`, `view_with_tilt_preserves_determinant`, `tilt_360_equals_zero`)
  - All offset tests (`mvp_with_zero_offset_unchanged`, `mvp_with_positive_x_offset_shifts_right`, `mvp_with_offset_preserves_depth`)
  - All zoom tests
  - All eye position tests

- **Test cases**: (enumerated above -- 7 new tests replacing 7 old tests)
- **Verify**: Tests exist and fail because `view_matrix()` still uses post-view rotations. The kept tests still pass.
- **Complexity**: Small

#### Step 1.2: Implement look-at target offset in `view_matrix()` (GREEN)

- **Files**: `src/scene/camera.rs` (lines 107-118)
- **Action**: Replace the current `view_matrix()` implementation:

  Current (post-view rotation):

  ```rust
  pub fn view_matrix(&self) -> Mat4 {
      let eye = self.eye_position();
      let center = glam::Vec3::ZERO;
      let up = glam::Vec3::Y;
      let base_view = Mat4::look_at_rh(eye, center, up);

      let tilt = Mat4::from_rotation_z(self.tilt_deg.to_radians());
      let pitch = Mat4::from_rotation_x(self.pitch_deg.to_radians());
      let yaw = Mat4::from_rotation_y(self.yaw_deg.to_radians());

      tilt * pitch * yaw * base_view
  }
  ```

  New (look-at target offset):

  ```rust
  pub fn view_matrix(&self) -> Mat4 {
      let eye = self.eye_position();
      let up = glam::Vec3::Y;

      // Compute local camera frame from eye toward origin
      let forward = (-eye).normalize();
      let right = forward.cross(up).normalize();
      let cam_up = right.cross(forward).normalize();

      // Shift look-at point from origin toward Earth's surface.
      // sin() maps slider degrees to offset: 0 deg -> center, 90 deg -> Earth surface (radius 1.0)
      let target = glam::Vec3::ZERO
          + right * self.yaw_deg.to_radians().sin()
          + cam_up * self.pitch_deg.to_radians().sin();

      let base_view = Mat4::look_at_rh(eye, target, up);

      // Tilt stays as post-view rotation (roll around forward axis)
      let tilt = Mat4::from_rotation_z(self.tilt_deg.to_radians());
      tilt * base_view
  }
  ```

  Update the doc comment on `view_matrix()` to describe the new approach: yaw/pitch shift the look-at target via `sin()` mapping, tilt remains a post-view Z rotation.

- **Verify**: `cargo test camera` passes -- all new tests from Step 1.1 pass, all kept tests pass.
- **Complexity**: Small

#### Step 1.3: Run full test suite and clippy

- **Files**: N/A
- **Action**: Run `cargo test` to verify no regressions outside the camera module. Run `cargo clippy` to verify no new warnings.
- **Verify**: `cargo test` passes. `cargo clippy` passes.
- **Complexity**: Small

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
| --- | --- | --- | --- |
| `view_with_zero_yaw_pitch_unchanged` | Unit | yaw=0, pitch=0 | Same matrix as default camera |
| `view_with_pitch_shifts_look_target_up` | Unit | pitch=30, point at (0, 0.5, 0) | Point is more centered in view-space Y |
| `view_with_yaw_shifts_look_target_right` | Unit | yaw=30, point at (0.5, 0, 0) | Point is more centered in view-space X |
| `pitch_90_looks_at_surface` | Unit | pitch=90 | View direction has significant upward component |
| `view_with_negative_yaw_mirrors_positive` | Unit | yaw=30 vs yaw=-30 | X deltas have opposite signs |
| `view_with_negative_pitch_mirrors_positive` | Unit | pitch=30 vs pitch=-30 | Y deltas have opposite signs |
| `yaw_pitch_preserves_determinant` | Unit | yaw/pitch in [-90, -45, 0, 45, 90] | Determinant > 0.5 |
| `view_with_zero_rotations_unchanged` (kept) | Unit | tilt=0, yaw=0, pitch=0 | Same as base view |
| `all_rotations_zero_equals_no_rotation` (kept) | Unit | tilt=0, yaw=0, pitch=0 | Same as base view |
| All tilt tests (kept) | Unit | Various tilt values | Unchanged behavior |
| All offset/zoom/eye tests (kept) | Unit | Various values | Unchanged behavior |

### Manual Verification

- [ ] Moving yaw slider shifts the visible portion of the Earth horizontally (camera looks to the side)
- [ ] Moving pitch slider shifts the visible portion of the Earth vertically (camera looks up/down)
- [ ] Effect is more dramatic at close zoom, subtler at far zoom
- [ ] At yaw=0, pitch=0, view is identical to before
- [ ] Tilt still works correctly (roll around forward axis)
- [ ] Yaw/pitch combined with tilt and offset produce sensible results
- [ ] Mouse drag behavior is unaffected (drag still rotates longitude/latitude correctly)

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| At extreme latitude (near poles), the `right` vector computation could degenerate because `forward` and `up` become parallel | NaN in view matrix at high latitudes with yaw/pitch | The existing latitude clamp at +/-89.9 degrees prevents exact alignment. At 89.9 degrees latitude, `forward.cross(up)` still has a non-zero component. Verify with the determinant test at various latitudes. |
| The `sin()` mapping means yaw/pitch=90 shifts the look-at point by exactly 1.0 (Earth's radius), which points the camera at the surface limb | At 90 degrees, the camera looks tangent to the Earth's surface, which may look odd | This is the intended maximum. The `sin()` saturation means values near 90 produce diminishing returns, naturally discouraging extremes. |

## Rollback Strategy

Revert the single commit. The change is entirely within `view_matrix()` and its unit tests in `camera.rs`. No other files are modified.

## Status

- [x] Plan approved
- [x] Implementation started
- [x] Implementation complete
