# Unsafe Code Audit - Research

## Overview

This document analyzes all `unsafe` code usage in the
sunlit-earth codebase, classifying each as **necessary**,
**replaceable** (safe crate exists), or **consolidatable**
(can be isolated in a dedicated boundary module).

The project enforces `unsafe_code = "deny"` in `Cargo.toml`
with scoped `#[allow(unsafe_code)]` on individual call sites.
All existing unsafe blocks have `// SAFETY:` comments.

## Original Inventory

**19 unsafe blocks across 6 files:**

| File               | Blocks | Category                        |
| ------------------ | ------ | ------------------------------- |
| `src/scene/sun.rs` | 5      | C FFI (astronomy engine)        |
| `src/wallpaper.rs` | 8      | Win32 FFI (monitor, reg, wp)    |
| `src/tray.rs`      | 3      | Win32 FFI (msg pump, thread ID) |
| `src/memory.rs`    | 1      | Win32 FFI (process memory)      |
| `src/config.rs`    | 1      | Win32 FFI (monitor validation)  |
| `src/main.rs`      | 1      | Win32 FFI (console attachment)  |

## Implemented Changes

### Registry FFI replaced with `winreg` (5 blocks removed)

The `ensure_fill_style()` and `set_reg_string()` functions
used raw Win32 registry API (`RegOpenKeyExW`,
`RegSetValueExW`, `RegCloseKey`) across 5 unsafe blocks.

Replaced with the `winreg` crate (v0.56.0), which provides
safe Rust wrappers with RAII handle management. The
`set_reg_string` helper was deleted entirely. Added
`winreg = "0.56"` to `[target.'cfg(windows)'.dependencies]`
and removed the `Win32_System_Registry` feature from
`windows-sys`.

### Tray thread sync replaced with channels (2 blocks removed)

The `PostThreadMessageW` + `GetCurrentThreadId` pattern
used 2 unsafe blocks plus two global atomics
(`TRAY_THREAD_ID`, `TRAY_AUTO_REFRESH`) to sync the
auto-refresh checkmark between the UI and tray threads.

Replaced with a `crossbeam-channel` pair (already a
transitive dependency via `tray-icon`). A single
`OnceLock<Sender<bool>>` replaces both atomics. The
enabled state travels through the channel directly,
and the message pump checks `rx.try_recv()` instead of
matching `WM_USER`. Added `crossbeam-channel = "0.5"`
as an explicit dependency.

## Remaining Unsafe (12 blocks across 5 files)

### `sun.rs` - 5 blocks - CONSOLIDATABLE

All 5 blocks call pure C functions from
`astronomy-engine-bindings` (raw bindgen FFI):

- `Astronomy_CurrentTime()` - get UTC time
- `Astronomy_MakeObserver()` - create observer struct
- `Astronomy_Equator()` - compute sun RA/Dec
- `Astronomy_SiderealTime()` - compute sidereal time
- `Astronomy_MakeTime()` - create time from calendar

All are pure with no preconditions and value-type returns.
A `sun_ffi.rs` wrapper module could isolate the FFI
boundary, making `sun.rs` itself 100% safe. This scales
naturally as more celestial body features are added.

### `wallpaper.rs` - Monitor enum (3 blocks) - NECESSARY

`get_primary_monitor_resolution()` uses
`EnumDisplayMonitors` with a callback-and-LPARAM pattern
that is inherently unsafe. Well-encapsulated and correct.

### `wallpaper.rs` - Set wallpaper (2 blocks) - NECESSARY

`SystemParametersInfoW(SPI_SETDESKWALLPAPER, ...)` and
`GetLastError()`. No way to set Windows wallpaper without
this Win32 API.

### `tray.rs` - Message pump (1 block) - NECESSARY

`GetMessageW()` / `TranslateMessage()` /
`DispatchMessageW()` plus `mem::zeroed()` for the MSG
struct. The tray-icon crate requires a Win32 message
pump and provides no safe wrapper.

### `config.rs` - MonitorFromRect (1 block) - NECESSARY

Validates saved window geometry is on a connected
monitor. Single trivial FFI call with no preconditions.

### `main.rs` - AttachConsole (1 block) - NECESSARY

Re-attaches to parent console for `--help` output.
Single trivial FFI call with no preconditions.

### `memory.rs` - Process memory (1 block) - NECESSARY

`GetProcessMemoryInfo` for RSS and private bytes.
The `sysinfo` crate could replace this but adds ~15
transitive dependencies for a single debug-logging call.

## Summary

| Category       | Blocks | Status              |
| -------------- | ------ | ------------------- |
| REPLACED       | 7      | Done (winreg, chan) |
| CONSOLIDATABLE | 5      | Future (sun_ffi.rs) |
| NECESSARY      | 7      | Keep as-is          |

**Before:** 19 unsafe blocks across 6 files
**After:** 12 unsafe blocks across 5 files
