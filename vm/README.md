# VM templates

Image-build inputs for the local desktop e2e VMs. The templates live here; the multi-gigabyte images they produce live outside the repo, in the image store (`%LOCALAPPDATA%\SunlitEarth\vm` or `~/.local/share/SunlitEarth/vm`, overridable with `SUNLIT_EARTH_VM_DIR`).

```
vm/
  linux/     Ubuntu 22.04 with GNOME on Xorg, autologin, OpenSSH
  windows/   Windows 11 Enterprise evaluation, autologon, OpenSSH, scheduled-task job runner
```

Nothing in here is compiled, and nothing is read at run time. The unit tests do read these files: they parse every script to check it is syntactically valid, and they check that the roots and device models named here match the ones the orchestrator uses, because nothing else connects the two. `cargo xtask vm build-image <target>` drives Packer over these files, and records the hash of the whole directory in the image's manifest. Editing anything under `vm/<target>/` therefore makes that target's image report as stale on the next `cargo xtask vm doctor`, which is the intended signal: rebuild when the template changes.

`docs/vm-setup.md` is the human guide to the whole flow.
