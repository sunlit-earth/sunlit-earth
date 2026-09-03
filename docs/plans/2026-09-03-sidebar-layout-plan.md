# Plan: sidebar layout (2026-09-03)

## Summary

The settings sidebar breaks when the Advanced section is opened on Linux: the whole panel is pushed right by about 40 pixels and the same amount is cut off the right edge, with no scrollbar to reach it. The cause is two independent Slint behaviors that only bite together, and only when the panel's content is wider than the panel.

The fix is in four parts, and only the first is a bug fix. The rest are what makes the class of defect impossible rather than this instance of it: nothing in the panel may report an unbounded minimum width, the panel's own floor is derived from its content instead of hardcoded, and a horizontal scrollbar catches anything that still gets through.

Four rearrangements ride along, since they touch the same block: the About button moves out of the scroll area and is pinned to the bottom of the sidebar, the Presets grid moves above Displays and is reordered, the Rendering group moves to the top of Advanced, and the panel reserves the vertical scrollbar's width so the bar stops covering the content.

## Stakes

Low. `ui/main.slint` and one number in `ui_callbacks.rs`, nothing in `sunlit-core`, no shader, no persisted setting, no IPC contract. The risk is that a layout change looks fine on the developer's machine and not on someone else's, which is exactly what happened here, so the deliverable includes one number a test can assert rather than a screenshot somebody looked at.

## The defect

Two things combine.

### The phantom offset

`main.slint:327` reads `width: parent.visible-width` on the `VerticalLayout` inside the sidebar's `ScrollView`, and sets no `x`.

Slint's `default_geometry` pass gives every element that is not a layout child, has no `x`, and whose width is not *literally* `100%` a centering binding, `x: (parent.width - self.width) / 2` (`i-slint-compiler/passes/default_geometry.rs:143`, and `maybe_center_in_parent` at line 489). `fix_percent_size` recognizes two spellings as "fills the parent": the literal `100%`, and `parent.<same-property>`. `parent.visible-width` is neither, so the centering applies.

The parent here is the flickable's viewport, not the flickable. `passes/flickable.rs:157` binds `viewport-width` to `max(flickable.width, <child layout>.min-width)`. So with content minimum `M` and visible width `W`:

- `M <= W`: the viewport is `W` wide, the offset evaluates to zero, nothing is wrong.
- `M > W`: the viewport is `M` wide, the layout is still `W` wide, and it is pushed right by `(M - W) / 2`.

The Flickable clips at `W`, and `horizontal-scrollbar-policy: always-off` (`main.slint:322`) means the clipped part is unreachable. A narrower sidebar makes `(M - W) / 2` larger, which is why dragging the splitter left made it worse.

### The content is wider than the panel

`M` exceeds `W` only when Advanced is open, because of `main.slint:1371`:

```slint
Text {
    text: renderer-info;
    color: #888888;
    font-size: 11px;
}
```

A `Text` with the default `wrap: no-wrap` reports its full unwrapped width as its minimum (`i-slint-core/items/text.rs:804`), and nothing caps it. `renderer-info` is built in `wgpu_init.rs:77` as `format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type)`. On the development machine that is `AMD Radeon 780M Graphics (RADV PHOENIX) (Vulkan, IntegratedGpu)`, 363 logical pixels in Noto Sans at 11px.

This is not a platform difference in Slint. It is the length of a runtime string. The Direct3D adapter name is shorter than the Mesa one, so Windows stays under the default 300px sidebar and looks fine.

### What was measured

Measured on the real `MainWindow` under `i-slint-backend-testing`, sidebar at its default 300px, by exposing `panel.min-width` and `panel.x`:

| state | `panel.min-width` | `panel.x` |
|---|---|---|
| Advanced closed | 176px | 0 |
| Advanced open, this machine's adapter string | 379px | **39.5px** |
| Advanced open, a short adapter string | 268px | 0 |
| Advanced open, with the fixes below | 176px | 0 |

`379 = 363 + 16` (the text plus the panel's 8px padding on each side), and `39.5 = (379 - 300) / 2`, so the arithmetic above is the whole story.

The third row is the one worth keeping: Windows is not safe, it is 32px away from the same failure. Drag the splitter to its 230px minimum there with Advanced open and it breaks too, adapter string or not.

The contributors to `M`, largest first:

| contributor | minimum width |
|---|---|
| `Text { text: renderer-info }`, no wrap | 363px |
| `SettingCombo` row: 88px label + 4px spacing + the fluent `ComboBox`'s hard 160px | 252px |
| `SettingCheck { text: "Auto-refresh wallpaper" }` | 160px |
| `SettingRow`: 88 + 4 + 4 + 45 | 141px |

Everything else already carries an explicit `min-width`, and an explicit constraint *replaces* the implicit one rather than being maxed with it (`passes/default_geometry.rs:317`), which is why the long slider labels are harmless today.

## The fix

### 1. Remove the phantom offset

`width: 100%` instead of `width: parent.visible-width`, at `main.slint:327` and again at `main.slint:1505` in `AboutWindow`, which has the same latent shape.

Inside a Flickable, `100%` already resolves against the flickable rather than the viewport (`passes/default_geometry.rs:364`), so the width is unchanged and only the centering goes away. Measured: `panel.x` 39.5 to 0.

This alone is not the fix. Without part 2 the content is flush left and still clipped on the right.

### 2. Bound every child's minimum width

The panel's minimum is a `max` over its children, so one unbounded child sets the floor. Three classes, and the pinning belongs in the `Setting*` wrapper components so a future widget can only escape it by bypassing them:

- **Dynamic text.** `wrap: word-wrap` takes a `Text`'s minimum to zero. Mandatory for any string that arrives from Rust. `overflow: elide` does *not* help: `layout_info` reads `wrap` and nothing else.
- **ComboBox.** `min-width: 0` on the instance overrides the widget's own `min-width: max(160px, ...)`. Instance bindings beat the inlined component root (`passes/inlining.rs:271`), verified by compiling it. Whether to actually take it to zero is decided under "the floor" below.
- **Button and CheckBox.** Their minimum is the label width. The preset grid already sets `min-width: 0`; the free-standing buttons do not.

For this change only the `renderer-info` text needs `wrap`. The rest is convention, written down so the next widget follows it.

### 3. Make the panel's floor its content's floor

`clamp(self.x + delta, 230px, root.width - 240px)` at `main.slint:1394` is a number with no relationship to the content. Replace it with the content's own minimum, which is readable: `min-width` on a layout element resolves to the computed `layoutinfo-h.min` (`passes/materialize_fake_properties.rs:186`), so it is exactly the number wanted and it maintains itself.

```slint
property <length> panel-width: 300px;          // what the drag stores
sidebar := Rectangle {
    width: max(panel.min-width, min(root.panel-width, root.width - 240px));
    ...
}
splitter := Splitter {
    x: sidebar.width;
    moved(delta) => { root.panel-width = root.panel-width + delta; }
}
```

The floor is applied outermost so the two bounds can never invert on a narrow window. Tested for a binding loop (`panel.width` from `sidebar.width` from `panel.min-width`) and there is none: horizontal layout info does not depend on the assigned width. In the probe, opening Advanced widened the sidebar from 230 to 379 on its own, with `panel.x` staying at 0.

Two things to know when implementing:

- A layout's `min-width` counts an `if`-gated subtree only once that repeater has materialized. A running app's layout pass does it; a test has to force it, which the existing tests already do by looking an element up with `find_by_element_id`.
- Give `MainWindow` a literal `min-width` so the image area cannot be squeezed to nothing by a wide panel. A binding from `panel.min-width` would be neater and risks a loop through the window's own layout, so start with a literal and only get clever if it is wrong.

**The resulting floor.** With the reserved scrollbar width from part 4 the panel's horizontal padding is 30px, so the floor lands at 282px, set by a `SettingCombo` row (88 + 4 + 160 + 30). That is 52px wider than today's 230px minimum. The knob if that feels too wide is a smaller explicit `min-width` on the `SettingCombo`'s `ComboBox`: 130px puts the floor at 252, and 100px puts it at 222, at the cost of the current value eliding in the narrow combos. Decide by looking at it; the point of this part is that the number stops being a guess either way.

### 4. Reserve the scrollbar's width

The fluent `ScrollView` gives the flickable the full width and draws `vertical-bar` on top of it at `width: 14px`. So the bar does not reflow anything, it *covers* the right 14px of the content, which is why slider values disappear under it once Advanced makes the panel scroll.

Reserve it in the panel's padding: `padding: 8px; padding-right: 22px;`. The specific property overrides the general one, verified: a child came out at `x = 8` with `width = 300 - 8 - 22 = 270`.

Nothing about this is conditional on whether the bar is visible, which is the point. The 14px is the fluent style's own constant and should be named where it is used.

### 5. A net under it

`horizontal-scrollbar-policy: always-off` at `main.slint:322` is what turns "too wide" into "silently invisible". `as-needed` makes anything that still gets through reachable. It should never trigger once parts 2 and 3 hold, which is exactly why it belongs there. Note that a horizontal bar would overlay the bottom row the same way the vertical one overlays the right edge; that is acceptable for something that is not supposed to appear.

## The rearrangement

### About pinned to the bottom

The About button (`main.slint:1377`) leaves the Advanced section and becomes a static footer, so it is reachable without opening Advanced and without scrolling. The sidebar rectangle gains a `VerticalLayout` holding the `ScrollView` above and the button below:

```slint
sidebar := Rectangle {
    VerticalLayout {
        ScrollView { vertical-stretch: 1; ... }
        HorizontalLayout { padding: 8px; Button { text: "About"; ... } }
    }
}
```

Verified: the `ScrollView` takes the remaining height and the button sits at the bottom edge, unaffected by how tall the scrolled content is.

The `renderer-info` text stays where it is, inside Advanced, now wrapped. Pairing it with the About row would also be defensible and is not part of this.

### Presets above Displays, reordered

The Presets block (`main.slint:459` to `533`) moves above the Displays block (`main.slint:397`). New grid order, left to right and top to bottom:

| | | |
|---|---|---|
| Africa | N. America | S. America |
| Asia | Europe | Oceania |
| Pacific | Blue Marble | Earthrise |

Only the buttons move. `PRESETS` in `scene/camera.rs:49` keeps its indices, so each button keeps the index it passes to `apply-preset` (Africa 3, N. America 1, S. America 2, Asia 4, Europe 0, Oceania 5, Pacific 6, Blue Marble 7, Earthrise 8). That keeps `all_presets_produce_valid_mvp`, any stored config, and the two label-keyed UI tests, which look the button up by name and assert the index it fires, working untouched. The doc comment on `PRESETS` names the old order and needs a line saying the array order and the grid order are deliberately different.

### Rendering first in Advanced

The Rendering `GroupBox` (`main.slint:1340`) moves to the top of the Advanced section, above Camera Position (`main.slint:562`). Pure reordering, no binding changes. `docs/architecture.md:88` lists the group order and has to follow.

## Tests

The regression guard is one number, not a screenshot. Add to `MainWindow`:

```slint
out property <length> panel-min-width: panel.min-width;
```

and to `tests/slint_ui.rs`, with Advanced open and the repeaters forced the way the existing tests force them:

- `panel_min_width` stays at or below the sidebar floor, with a long `renderer_info` set on the window so the test exercises the case that broke rather than the case that happened to work. This is the one that fails when somebody adds a wide widget.
- The panel's `x` is 0 in both Advanced states, which pins part 1 directly.

Also worth having, cheap:

- The About button is findable with Advanced closed. It was not, before this.
- The nine preset buttons still fire the indices they are named for, which the two existing tests already half-cover; extend to all nine now that the order is being changed by hand.

Note in the test file that `min-width` is only meaningful after the repeaters have been walked, or the assertion passes for the wrong reason.

## Docs

- `docs/architecture.md:88`: the Advanced group order changes, the About button is no longer in that section, and the paragraph below it about the `Setting*` components gains the min-width convention from part 2.
- `docs/roadmap.md`: an entry for the fix, with the measured numbers and the reason Windows was never safe either.

## What is not in this

- Reworking the splitter into a real `HorizontalLayout` where the window's own minimum width follows the panel's. That is the structurally correct answer and a bigger change than the defect justifies.
- Making `renderer-info` shorter or moving it into the About window. The wrap makes it harmless where it is.
- The `AboutWindow` beyond the one-token `width: 100%`. Its texts are short and its own floor is not interesting.
- Any change to `PRESETS` itself, its values, or its indices.

## Order of work

1. Part 1 and the `wrap` from part 2, which together fix the reported bug. Confirm `panel.x` is 0 and the panel minimum drops to 176.
2. Part 4, then part 3, in that order: the reserved padding changes the floor, so deriving the floor afterwards saves doing it twice.
3. Part 5.
4. The four rearrangements.
5. The tests, then `cargo test` and `cargo clippy --all-targets`.
6. The docs.
