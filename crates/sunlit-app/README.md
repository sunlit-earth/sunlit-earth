# sunlit-app

The application shell around [sunlit-core](../sunlit-core/): the Slint settings window, the system tray icon, the IPC control channel, and the CLI. The package is named `sunlit-earth`, so this directory builds the `sunlit-earth` binary.

The shell stays thin on purpose. It reads sliders into `SceneParams` and sends them to the engine (`ui_callbacks.rs`, `engine_client.rs`); preview frames come back as plain pixel buffers, so Slint never holds a GPU object. `main.rs` has two paths: `run_app` opens the window and event loop, and `run_render` is fully headless, which is what makes `cargo run -- render --output x.png` work on machines with no desktop at all.

`ipc.rs` is an opt-in local-socket channel (`--ipc-socket <name>`) used by the e2e suite: `quit`, `show-window`, `hide-window`, `export-test`, `set-wallpaper`, `query-memory`.

## Tests

```bash
cargo test -p sunlit-earth        # unit + slint_ui (testing backend, no desktop)
cargo e2e                         # desktop e2e: spawns the real binary, needs a desktop
cargo xtask e2e --target linux    # the same suite inside a local VM
```

The e2e suite opens real windows and a real tray icon; run it on a desktop you are willing to share for a minute, or in a VM per [docs/vm-setup.md](../../docs/vm-setup.md).
