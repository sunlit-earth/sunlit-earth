# sunlit-ui

The Slint sources of the settings window, the About window and the tray, under `ui/`, and the `slint-build` step that compiles them. The library is only `slint::include_modules!()`, and [sunlit-app](../sunlit-app/) re-exports it, so the generated types are reached as `sunlit_earth::MainWindow` and so on. The callbacks, the models and the window bridge live in sunlit-app.

The crate exists for build times: the code Slint generates is about 10 MB of Rust, and in a crate of its own it is compiled again only when a `.slint` file changes. See [docs/architecture.md](../../docs/architecture.md) for the figures and the UI itself.
