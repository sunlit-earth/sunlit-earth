# VM templates

Image-build inputs for the local VMs. The templates live here; the multi-gigabyte images they produce live outside the repo, in the image store (`%LOCALAPPDATA%\SunlitEarth\vm` or `~/.local/share/SunlitEarth/vm`, overridable with `SUNLIT_EARTH_VM_DIR`).

```
vm/
  linux/            Debian 13 with four desktops on Xorg, sddm autologin, OpenSSH
  windows/          Windows 11 Enterprise evaluation, autologon, OpenSSH, scheduled-task job runner
  linux-builder/    Ubuntu 22.04, a Rust toolchain, no graphics stack at all
  windows-builder/  the two scripts that turn a child of `windows/`'s image into a builder
```

One directory per image, and the directory name is the image's slug: `cargo xtask vm build-image <slug>`. The first two are the desktop guests the e2e suite runs in; the second two are where `cargo xtask dist` builds a release binary.

The Linux image carries KDE Plasma, GNOME, XFCE and Cinnamon side by side, and nothing in the image decides which one a boot uses: the host names a session through QEMU's fw_cfg and a oneshot unit writes it into sddm's autologin configuration before the display manager starts. So `cargo xtask vm up linux --desktop gnome` costs a boot rather than a rebuild. `desktop.sh` holds the allowlist of session names, and a unit test compares it against the four the host can ask for.

The two builders are deliberately unlike the guests they build for. Neither has a desktop, an X server, Mesa or a display manager, because a builder is not something the suite can run in and a compiler in the images the suite *does* run in would cost the fidelity that found the missing Visual C++ runtime: the guest that found it was a stock Windows. `linux-builder` is Ubuntu 22.04 rather than the Debian 13 next to it, because the glibc floor a binary carries is the glibc of the machine that linked it, and 22.04's 2.35 runs on everything since while trixie's 2.41 would refuse Ubuntu 24.04.

`windows-builder/` has no Packer template, only scripts. It is a *layer*: a differencing child of the Windows image's own disk, provisioned over SSH by `commands::build_layer`, which is what makes it minutes and a few gigabytes rather than an hour and another fifteen. The consequence is that it cannot be read without its parent, so its manifest records the parent's checksum and the two are rebuilt and purged together.

Nothing in here is compiled, and nothing is read at run time. The unit tests do read these files: they parse every script under every `<slug>/scripts/` to check it is syntactically valid, and they check that the roots, device models and toolchain paths named here match the ones the orchestrator uses, because nothing else connects the two. `cargo xtask vm build-image <slug>` records the hash of the whole directory in the image's manifest, so editing anything under `vm/<slug>/` makes that image report as stale on the next `cargo xtask vm doctor`, which is the intended signal: rebuild when the template changes.

`docs/vm-setup.md` is the human guide to the whole flow.
