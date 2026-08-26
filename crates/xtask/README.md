# xtask

Developer tooling, invoked as `cargo xtask` through the alias in `.cargo/config.toml`. Not part of the shipped application. Its job is running the desktop e2e suite in local virtual machines, so the suite stops taking over the developer's desktop: host preparation, golden image builds with Packer, and the VM lifecycle over Hyper-V and QEMU.

```bash
cargo xtask vm doctor              # can this host run the VM suite? read-only
cargo xtask vm setup               # the one command that changes the host
cargo xtask vm build-image <t>     # golden image via Packer
cargo xtask e2e --target <t>       # the suite: host, windows, or linux
cargo xtask vm status              # what exists and what it costs on disk
cargo xtask vm destroy <t> [--purge]
```

`docs/vm-setup.md` is the human guide; `docs/plans/2026-08-19-phase3-vm-orchestration-plan.md` is the design.

The sources are grouped by domain: `commands/` (one thin module per subcommand), `host/` (what this machine offers), `store/` (the image store, manifests, and expiry), `provider/` (the hypervisor boundary and the target matrix), `guest/` (artifacts, job scripts, and ssh plumbing). Two rules shape all of it: every process interaction goes through `runner::Runner`, and every decision is a pure function over data, so `cargo test -p xtask` exercises the logic against fabricated hosts with no hypervisor, no elevation, and no VM.
