# Research: Non-Blocking Background Texture Loading (2026-03-15)

## Problem Statement

The current texture loading in `ensure_bind_group_loaded()` (renderer.rs:312-368) is synchronous: when the user selects a new texture from the combobox, JXL decoding and GPU upload happen inside the `BeforeRendering` callback, blocking the UI thread. For 8K textures, JXL decoding alone takes several seconds, during which the window is completely unresponsive.

The goal is to move the CPU-heavy image decoding to a background thread while keeping the UI responsive, then perform GPU resource creation either on the background thread (if safe) or back on the UI thread.

## Current Architecture

### Threading Model

The application is single-threaded. Slint owns the event loop and drives all rendering through `set_rendering_notifier()`, which fires three callback states on the UI thread:

- `RenderingSetup` -- create GPU resources
- `BeforeRendering` -- update uniforms, render the frame, convert texture to Slint Image
- `RenderingTeardown` -- drop GPU resources

### GPU Resource Storage

GPU resources live in a `thread_local! { RefCell<Option<GpuResources>> }` (renderer.rs:115-117). This is necessary because the rendering notifier closure must be `'static`. The `GpuResources` struct contains the wgpu `Device` and `Queue` as owned fields, along with the pipeline, buffers, textures, and a `Vec<TextureSlot>` for lazy-loaded bind groups.

### Current Lazy Loading Flow

```
User selects texture in combobox
  -> on_texture_changed callback
  -> window.request_redraw()
  -> BeforeRendering fires
  -> ensure_bind_group_loaded() called with slot_index
  -> if bind_group is None:
       texture_loader::load(&path)        // CPU: JXL decode, several seconds for 8K
       create_mipmapped_texture()          // GPU: texture + mipmap upload
       create_bind_group()                 // GPU: bind group creation
       store in texture_slots[index]
  -> render frame with the bind group
```

The entire decode + upload happens synchronously, blocking the UI thread.

### Fallback Behavior

The renderer already has `last_rendered_index` tracking: when a requested texture slot isn't loaded, it renders using the most recently successful texture. This is exactly the behavior we want during async loading -- show the previous texture while the new one loads in the background.

## API Research

### wgpu Threading Guarantees

All key wgpu types are `Send + Sync`:

| Type | Send | Sync | Source |
|------|------|------|--------|
| `Device` | Yes | Yes | [docs.rs/wgpu/28/Device](https://docs.rs/wgpu/28.0.0/wgpu/struct.Device.html) |
| `Queue` | Yes | Yes | [docs.rs/wgpu/28/Queue](https://docs.rs/wgpu/28.0.0/wgpu/struct.Queue.html) |
| `Texture` | Yes | Yes | [docs.rs/wgpu/28/Texture](https://docs.rs/wgpu/28.0.0/wgpu/struct.Texture.html) |
| `BindGroup` | Yes | Yes | [docs.rs/wgpu/28/BindGroup](https://docs.rs/wgpu/28.0.0/wgpu/struct.BindGroup.html) |
| `BindGroupLayout` | Yes | Yes | (inherits from Device's internal synchronization) |
| `Sampler` | Yes | Yes | (inherits from Device's internal synchronization) |
| `Buffer` | Yes | Yes | (inherits from Device's internal synchronization) |

This means:

1. `Device` and `Queue` can be cloned (they are `Arc`-wrapped internally) and sent to background threads.
2. `create_texture()`, `write_texture()`, `create_bind_group()` can all be called from any thread.
3. A `BindGroup` created on a background thread can be sent back to the UI thread and used in a render pass.

**Key insight**: wgpu resource creation is thread-safe by design. We are not limited to creating GPU resources on the UI thread. The entire pipeline -- texture creation, mipmap upload, and bind group creation -- can happen on the background thread.

### Slint Threading APIs

**`slint::invoke_from_event_loop(func)`**
- Signature: `fn invoke_from_event_loop(func: impl FnOnce() + Send + 'static) -> Result<(), EventLoopError>`
- Queues a closure for execution on the UI thread. Thread-safe, can be called from any thread.
- The closure must be `Send + 'static`.
- [docs.rs/slint/1.15/invoke_from_event_loop](https://docs.rs/slint/1.15.0/slint/fn.invoke_from_event_loop.html)

**`Weak::upgrade_in_event_loop(func)`**
- Signature: `fn upgrade_in_event_loop(&self, func: impl FnOnce(T) + Send + 'static) -> Result<(), EventLoopError>`
- Combines upgrading a weak window reference with scheduling on the event loop.
- If the window was dropped, the closure is not called.
- `slint::Weak<T>` is `Send + Sync`, so it can be captured by background threads.
- [docs.rs/slint/1.15/Weak/upgrade_in_event_loop](https://docs.rs/slint/1.15.0/slint/struct.Weak.html#method.upgrade_in_event_loop)

**`Window::request_redraw()`**
- Tells Slint to schedule a new `BeforeRendering` callback.
- `Window` is `!Send + !Sync`, so this must be called from the UI thread.
- Can be called from within `invoke_from_event_loop` or `upgrade_in_event_loop`.

### Standard Library Concurrency Primitives

**`std::sync::mpsc::channel`** -- multi-producer, single-consumer channel. The `Receiver` is `!Sync` but is `Send`, so it can be owned by one thread. The `Sender` is `Send + Sync` and can be cloned.

**`std::sync::mpsc::Receiver::try_recv()`** -- non-blocking check for a message. Returns `Ok(value)`, `Err(TryRecvError::Empty)`, or `Err(TryRecvError::Disconnected)`. This is ideal for polling in `BeforeRendering` without blocking.

**`std::sync::Arc<Mutex<T>>`** -- shared mutable state across threads. More general than channels but requires locking.

**`std::sync::atomic::AtomicBool`** -- lock-free flag for simple "is it ready?" signaling.

## Design Options

### Option A: Background Decode, UI Thread GPU Upload (Channel-Based)

```
User selects texture
  -> spawn std::thread:
       texture_loader::load(&path)           // CPU work on background thread
       send DecodedImage over channel
  -> invoke_from_event_loop:
       window.request_redraw()               // wake the UI
  -> BeforeRendering:
       try_recv() on channel
       if DecodedImage available:
         create_mipmapped_texture()           // GPU work on UI thread
         create_bind_group()                  // GPU work on UI thread
         store in texture_slots[index]
       render with last_rendered_index or new texture
```

**Pros**:
- Simple mental model: background thread does pure CPU work, UI thread does all GPU work
- No need to share Device/Queue across threads
- No risk of driver-level threading issues with GPU resource creation
- Channel is the natural Rust concurrency primitive for this producer/consumer pattern

**Cons**:
- GPU upload (mipmap generation + `write_texture` for all mip levels) still happens on the UI thread, which could cause a brief hitch for 8K textures (though much shorter than JXL decode)
- Two-phase handoff adds some complexity to the `BeforeRendering` callback

**Estimated UI-thread cost for GPU upload only**: Mipmap generation for an 8192x4096 RGBA texture is ~130 MB of pixel processing. The `downsample_2x` function is simple arithmetic. At modern CPU speeds this is roughly 50-100 ms. The `queue.write_texture()` calls for all mip levels copy data into staging memory (not GPU-bound). Total UI-thread stall for the GPU upload phase: likely under 200 ms, imperceptible compared to the multi-second JXL decode.

### Option B: Everything on Background Thread (Channel-Based)

```
User selects texture
  -> spawn std::thread with cloned Device + Queue:
       texture_loader::load(&path)           // CPU work
       create_mipmapped_texture()            // GPU work (on background thread)
       create_bind_group()                   // GPU work (on background thread)
       send BindGroup over channel
  -> invoke_from_event_loop:
       window.request_redraw()
  -> BeforeRendering:
       try_recv() on channel
       if BindGroup available:
         store in texture_slots[index]
       render
```

**Pros**:
- Zero UI-thread stall: the entire pipeline (decode + mipmap + GPU upload + bind group) runs on the background thread
- Simpler BeforeRendering: just check the channel and swap in the bind group

**Cons**:
- Requires sharing Device, Queue, BindGroupLayout, Sampler, and UniformBuffer references with the background thread (all are `Send + Sync` so this is safe, but adds `Arc` wrapping or cloning)
- The BindGroupLayout, Sampler, and UniformBuffer are currently owned by `GpuResources` in the thread_local. To share them with a background thread, they would need to be wrapped in `Arc` or cloned. Device and Queue are already internally `Arc`-wrapped and cheap to clone.
- Slightly more complex resource lifetime management

**Feasibility of sharing GPU resources**: `Device::clone()` and `Queue::clone()` are cheap (they clone inner `Arc`s). `BindGroupLayout`, `Sampler`, and `Buffer` are also `Send + Sync`. They could be wrapped in `Arc` inside `GpuResources`, or cloned if the types support it. Since wgpu types are reference-counted internally, cloning is cheap.

### Option C: Background Decode, `invoke_from_event_loop` for GPU Upload

```
User selects texture
  -> spawn std::thread:
       texture_loader::load(&path)           // CPU work
       invoke_from_event_loop:
         GPU_RESOURCES.with(|r| {
           create_mipmapped_texture()
           create_bind_group()
           store in texture_slots[index]
         })
         window.request_redraw()
```

**Pros**:
- No channel needed -- the background thread directly schedules the GPU work on the UI thread via `invoke_from_event_loop`
- Clean separation: background does CPU, closure on UI thread does GPU

**Cons**:
- The `invoke_from_event_loop` closure must be `Send + 'static`, which means the `DecodedImage` (a `Vec<u8>` + dimensions) must be moved into it. This is fine -- `Vec<u8>` is `Send`.
- The closure accesses the `thread_local!` GPU_RESOURCES, which works because `invoke_from_event_loop` runs on the UI thread.
- The GPU upload happens outside of `BeforeRendering`, which might interact with Slint's rendering pipeline in unexpected ways. Need to verify: can we safely call `queue.write_texture()` and `device.create_bind_group()` outside of the rendering notifier callback? Since wgpu operations are independent of Slint's rendering state, this should be safe -- the resources are just being prepared, and they'll be used in the next `BeforeRendering`.
- If `RenderingTeardown` fires while a background thread is in flight, the closure in `invoke_from_event_loop` would find `GPU_RESOURCES` is `None`. This needs a guard check.

### Option D: `Arc<Mutex<Option<BindGroup>>>` Shared Slot

```
User selects texture
  -> create Arc<Mutex<Option<BindGroup>>> (initially None)
  -> spawn std::thread with cloned Device + Queue + Arc:
       texture_loader::load(&path)
       create GPU resources
       *arc.lock() = Some(bind_group)
  -> BeforeRendering:
       if let Some(bg) = arc.lock().take():
         store in texture_slots[index]
       render
```

**Pros**:
- Very direct shared-state model

**Cons**:
- Mutex lock in `BeforeRendering` on every frame (even when not loading) -- though `try_lock()` could mitigate this
- More complex state management than a channel
- Need to manage the Arc lifetime across the thread_local boundary

## Recommended Approach: Option A (Background Decode, UI Thread GPU Upload)

Option A is the best fit for this project for the following reasons:

### Why Option A

1. **Simplest thread interaction**: The background thread does pure CPU work (JXL decode, horizontal flip, shift) and sends the result over a channel. No GPU types cross the thread boundary. No `Arc` wrapping of GPU resources needed.

2. **Minimal changes to renderer.rs**: The `GpuResources` struct doesn't need `Arc`-wrapped fields. The `ensure_bind_group_loaded` function is replaced with a two-part flow: (a) kick off background loading if needed, (b) check channel for results in `BeforeRendering`.

3. **Safe by construction**: `DecodedImage` is a `Vec<u8>` + two `u32` dimensions -- trivially `Send`. The channel provides clean ownership transfer. GPU resource creation stays entirely within the thread_local context.

4. **The GPU upload cost is negligible**: The bottleneck is JXL decoding (seconds), not GPU texture creation (milliseconds). Moving only the decode to a background thread eliminates ~95% of the UI stall. The remaining mipmap generation + upload is fast enough to be imperceptible.

5. **Existing fallback works perfectly**: While the background thread is decoding, `BeforeRendering` continues to render using `last_rendered_index`. No special "loading" UI state is needed.

### Why Not the Others

- **Option B**: Adds `Arc` complexity for marginal benefit (saving ~100-200 ms of UI-thread work that users won't notice). The shared GPU resource lifetime across the thread_local boundary is the biggest concern.
- **Option C**: `invoke_from_event_loop` doing GPU work outside of `BeforeRendering` is unusual and potentially fragile. The interaction with Slint's internal rendering state is undocumented.
- **Option D**: `Arc<Mutex>` is heavier machinery than needed for a simple producer-consumer pattern. A channel is the idiomatic Rust solution.

## Detailed Design for Option A

### Data Flow

```
                    UI Thread                          Background Thread
                    =========                          =================

on_texture_changed:
  set pending_slot_index = selected
  window.request_redraw()

BeforeRendering:
  1. Check channel: try_recv()
     If DecodedImage received:
       create_mipmapped_texture()
       create_bind_group()
       store in texture_slots[completed_index]

  2. Check if requested slot needs loading:
     If texture_slots[index].bind_group is None
        AND source_path is Some
        AND no load in flight for this index:
       Clone the path
       Send (index, path) to spawned thread  ------>  std::thread::spawn:
       Mark slot as "loading"                           texture_loader::load(&path)
                                                        Send (index, DecodedImage) back
  3. Resolve render index:                              invoke_from_event_loop:
     If requested slot loaded: use it                     window.request_redraw()
     Else: use last_rendered_index

  4. Render frame as usual
```

### New Types and Fields

```rust
/// Message sent from background thread to UI thread when decoding completes.
struct DecodedTextureMessage {
    slot_index: usize,
    image: texture_loader::DecodedImage,
}

/// Extended TextureSlot with loading state.
struct TextureSlot {
    bind_group: Option<wgpu::BindGroup>,
    source_path: Option<PathBuf>,
    loading: bool,  // true while a background thread is decoding this slot
}
```

Add to `GpuResources`:

```rust
/// Receiver end of the channel for completed texture decodes.
texture_rx: std::sync::mpsc::Receiver<DecodedTextureMessage>,
/// Sender end, cloned into each background decode thread.
texture_tx: std::sync::mpsc::Sender<DecodedTextureMessage>,
```

### Channel Setup

The channel is created in `create_gpu_resources()` and both ends are stored in `GpuResources`. The `Sender` is cloned and moved into each spawned thread. The `Receiver` is polled via `try_recv()` in `BeforeRendering`.

```rust
let (texture_tx, texture_rx) = std::sync::mpsc::channel();
```

### Modified `BeforeRendering` Flow

```rust
// Phase 1: Collect completed background decodes
while let Ok(msg) = res.texture_rx.try_recv() {
    let tex = create_mipmapped_texture(
        &res.device, &res.queue,
        &format!("texture_slot_{}", msg.slot_index),
        msg.image.width, msg.image.height, &msg.image.pixels,
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = create_bind_group(
        &res.device, &res.bind_group_layout,
        &res.uniform_buffer, &view, &res.sampler,
        &format!("bind_group_slot_{}", msg.slot_index),
    );
    res.texture_slots[msg.slot_index].bind_group = Some(bind_group);
    res.texture_slots[msg.slot_index].loading = false;
}

// Phase 2: Kick off background loading if needed
let slot_index = current_state.texture_index as usize;
if res.texture_slots[slot_index].bind_group.is_none()
    && !res.texture_slots[slot_index].loading
{
    if let Some(path) = res.texture_slots[slot_index].source_path.clone() {
        res.texture_slots[slot_index].loading = true;
        let tx = res.texture_tx.clone();
        let window_weak = window_weak.clone();  // for request_redraw
        std::thread::spawn(move || {
            texture_loader::register_jxl_hook();
            let start = std::time::Instant::now();
            match texture_loader::load(&path) {
                Ok(image) => {
                    eprintln!(
                        "Decoded texture ({}x{}) from {} in {:.2}s",
                        image.width, image.height,
                        path.display(), start.elapsed().as_secs_f64(),
                    );
                    let _ = tx.send(DecodedTextureMessage { slot_index, image });
                }
                Err(e) => {
                    eprintln!("{e}");
                    // Send a failure message or handle via a separate channel
                }
            }
            // Wake the UI to process the result
            let _ = window_weak.upgrade_in_event_loop(|win| {
                win.window().request_redraw();
            });
        });
    }
}

// Phase 3: Resolve render index (same as current logic)
let render_index = if res.texture_slots[slot_index].bind_group.is_some() {
    res.last_rendered_index = slot_index;
    slot_index
} else {
    res.last_rendered_index
};
```

### Passing `window_weak` to the Background Thread

The background thread needs a `slint::Weak<MainWindow>` to call `upgrade_in_event_loop` for `request_redraw()`. `slint::Weak` is `Send + Sync`, so it can be moved into the spawned thread.

Currently, `window_weak` is passed to `rendering_callback` as a `&slint::Weak<MainWindow>`. To clone it for the background thread, the reference is sufficient -- `Weak::clone()` is cheap.

The `window_weak` reference is already available inside the `BeforeRendering` branch. It would need to be cloned before being moved into the `std::thread::spawn` closure.

### Handling Decode Failures

When the background thread fails to decode, it should notify the UI thread so the slot can be marked as permanently failed (setting `source_path = None` and `loading = false`). Options:

1. **Send a `Result` through the channel**: Change the message type to carry `Result<DecodedImage, String>`. On the receiving end, handle the `Err` case by marking the slot failed.
2. **Separate failure channel**: Overly complex for this use case.
3. **Use `Option<DecodedImage>` in the message**: Send `None` on failure.

Option 1 is cleanest:

```rust
struct DecodedTextureMessage {
    slot_index: usize,
    result: Result<texture_loader::DecodedImage, String>,
}
```

### Handling Teardown During In-Flight Loads

If the user closes the window while a background thread is decoding:

1. `RenderingTeardown` fires, setting `GPU_RESOURCES` to `None`.
2. The background thread finishes decoding and calls `tx.send(...)`. The channel send will return `Err(SendError)` if the receiver has been dropped (it was inside `GpuResources`). This is harmless.
3. The background thread calls `window_weak.upgrade_in_event_loop(...)`. If the window is gone, the closure is not called. This is also harmless.
4. The background thread exits.

No cleanup is needed. The design is naturally safe against teardown races.

### Handling Rapid Texture Switching

If the user rapidly switches textures (e.g., Day -> Night -> Day), multiple background threads could be spawned. This is acceptable because:

- Each thread sends to the same channel with its `slot_index`, so results are routed correctly.
- The `loading` flag prevents re-spawning a thread for a slot that's already being decoded.
- If the user switches away from a texture before it finishes loading, the result still arrives and is stored -- it's ready for instant display if they switch back.
- At most N-1 background threads can be active (one per non-grid texture slot). With the current setup (Day + Night), that's at most 2 threads.

### Thread Lifetime

Using `std::thread::spawn` creates a detached thread. This is fine because:

- The thread does bounded work (decode one image) and exits.
- If the main thread exits first, the background thread is killed by `std::process::exit(0)` which is already at the end of `main()`.
- No join handle needs to be stored or awaited.

### Changes to `register_jxl_hook()`

The JXL hook registration uses a global (process-wide) hook in the `image` crate. It is documented as idempotent. It currently uses `std::sync::Once` or similar internally. It is safe to call from multiple threads, but to be safe, it should be called once from the main thread at startup (which already happens in `main.rs:49`) and then again from each background thread (belt-and-suspenders, since the hook is global and idempotent).

### No New Dependencies Required

The implementation uses only `std::thread`, `std::sync::mpsc`, and existing crate APIs. No async runtime (tokio, async-std) is needed. No new crate dependencies.

## Mipmap Generation Optimization (Future)

The current `downsample_2x` function processes pixels sequentially. For an 8K base texture, mipmap generation involves processing ~170 MB of pixel data across all levels. This is CPU work that could also be moved to the background thread in Option B. However, measuring first is important -- if the total UI-thread cost is under 200 ms, optimization is premature.

If mipmap generation proves to be a bottleneck, consider:

1. Moving to Option B (full background GPU upload) -- most impactful
2. Using `rayon` for parallel mipmap generation (already a dependency via jxl-oxide)
3. GPU-side mipmap generation via compute shaders (significant complexity increase)

## Summary of Findings

| Question | Answer |
|----------|--------|
| Can we spawn a `std::thread` to decode JXL, then do GPU upload on UI thread? | Yes. This is Option A, the recommended approach. `DecodedImage` is `Send`, channel transfer is clean. |
| Can we do GPU upload on the background thread too? | Yes. All wgpu types are `Send + Sync`. Option B is viable but adds unnecessary complexity for minimal gain. |
| How should the background thread communicate completion? | `std::sync::mpsc::channel` with `try_recv()` in `BeforeRendering`. Non-blocking, idiomatic Rust. |
| How do we trigger a redraw after background completion? | `Weak::upgrade_in_event_loop` from the background thread to call `window.request_redraw()`. |
| How do we handle the "loading" state in the UI? | The existing `last_rendered_index` fallback already handles this. While loading, the previous texture continues to render. No special loading indicator is needed. |
| What about teardown during in-flight loads? | Naturally safe: channel send fails harmlessly, `upgrade_in_event_loop` is a no-op if the window is gone. |
| What about rapid texture switching? | The `loading` flag prevents duplicate spawns. Multiple in-flight decodes for different slots coexist safely. |
| New dependencies needed? | None. `std::thread` + `std::sync::mpsc` from the standard library. |
| `unsafe` code needed? | No. The entire design uses safe Rust. |
