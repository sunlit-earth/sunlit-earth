# VM templates

Image-build inputs for the local desktop e2e VMs. The templates live here; the multi-gigabyte images they produce live outside the repo, in the image store (`%LOCALAPPDATA%\SunlitEarth\vm` or `~/.local/share/SunlitEarth/vm`, overridable with `SUNLIT_EARTH_VM_DIR`).

```
vm/
  linux/     Debian 13 with four desktops on Xorg, sddm autologin, OpenSSH
  windows/   Windows 11 Enterprise evaluation, autologon, OpenSSH, scheduled-task job runner
```

The Linux image carries KDE Plasma, GNOME, XFCE and Cinnamon side by side, and nothing in the image decides which one a boot uses: the host names a session through QEMU's fw_cfg and a oneshot unit writes it into sddm's autologin configuration before the display manager starts. So `cargo xtask vm up linux --desktop gnome` costs a boot rather than a rebuild. `desktop.sh` holds the allowlist of session names, and a unit test compares it against the four the host can ask for.

Nothing in here is compiled, and nothing is read at run time. The unit tests do read these files: they parse every script to check it is syntactically valid, and they check that the roots and device models named here match the ones the orchestrator uses, because nothing else connects the two. `cargo xtask vm build-image <target>` drives Packer over these files, and records the hash of the whole directory in the image's manifest. Editing anything under `vm/<target>/` therefore makes that target's image report as stale on the next `cargo xtask vm doctor`, which is the intended signal: rebuild when the template changes.

`docs/vm-setup.md` is the human guide to the whole flow.
