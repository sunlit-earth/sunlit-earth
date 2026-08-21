//! Getting the Windows evaluation ISO onto the host.
//!
//! Best effort with a clear manual fallback, which is what the plan's success
//! criterion 3 asks for. The download is a single pinned link followed through
//! its redirects, and anything that comes back too small to be an ISO is
//! treated as the error page it almost certainly is.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::runner::{Cmd, Runner, powershell, ps_quote};
use crate::store::Store;
use crate::util::format_bytes;

/// The Evaluation Center download page, for the manual route.
pub const EVAL_PAGE: &str =
    "https://www.microsoft.com/en-us/evalcenter/download-windows-11-enterprise";

/// The pinned link for Windows 11 Enterprise evaluation, x64, en-us.
///
/// Resolved 2026-08-19 through `go.microsoft.com/fwlink` to `aka.ms/Win11E-ISO-25H2-en-us`
/// and on to a `software-static.download.prss.microsoft.com` path holding
/// build 26200.6584 (media stamp 250915-1905), 7092807680 bytes. No session,
/// no cookie, no token: the redirect chain resolves for an anonymous client.
///
/// Which of the three links to pin is a judgement call. The `prss` URL embeds
/// the build and dies at the next revision; the `aka.ms` alias embeds the
/// release and dies at the next feature update; the fwlink id is the most
/// durable of the three, and is the one pinned here. Microsoft documents none
/// of them, so this is observed behavior rather than a contract, and the manual
/// fallback exists because of that.
pub const EVAL_FWLINK: &str =
    "https://go.microsoft.com/fwlink/?linkid=2334167&clcid=0x409&culture=en-us&country=us";

/// Anything smaller than this is not a Windows ISO. The real one is about
/// 6.6 GiB; an expired link or a form page comes back in kilobytes.
pub const MIN_PLAUSIBLE_ISO_BYTES: u64 = 3 * 1024 * 1024 * 1024;

/// Whether a downloaded file is plausibly the ISO rather than an error page.
pub fn plausible_iso(bytes: u64) -> bool {
    bytes >= MIN_PLAUSIBLE_ISO_BYTES
}

/// What to tell someone when the automated fetch did not work.
pub fn manual_instructions(destination: &Path) -> String {
    format!(
        "The Windows evaluation ISO could not be downloaded automatically.\n\n\
         Microsoft publishes it behind a registration form, and the direct links \
         are not documented, so this is expected to break from time to time.\n\n\
         Do this instead:\n  \
         1. Open {EVAL_PAGE}\n  \
         2. Download the Windows 11 Enterprise evaluation, 64-bit, English (United States)\n  \
         3. Save it as exactly this path:\n     {}\n  \
         4. Run `cargo xtask vm build-image windows` again\n\n\
         The build does not care which revision it is, but the autounattend file \
         in vm/windows/ was written against build 26200 (25H2).",
        destination.display()
    )
}

/// Make sure the ISO is in the store, downloading it if it is not.
pub fn ensure_iso(runner: &dyn Runner, store: &Store) -> Result<PathBuf, String> {
    let destination = store.windows_iso();
    if let Ok(meta) = std::fs::metadata(&destination) {
        if plausible_iso(meta.len()) {
            println!(
                "using the cached ISO at {} ({})",
                destination.display(),
                format_bytes(meta.len())
            );
            return Ok(destination);
        }
        println!(
            "the cached ISO is only {}, which is too small to be one; fetching it again",
            format_bytes(meta.len())
        );
        let _ = std::fs::remove_file(&destination);
    }

    if let Some(dir) = destination.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }

    let Some(command) = download_command(runner, EVAL_FWLINK, &destination) else {
        return Err(manual_instructions(&destination));
    };

    println!("downloading the Windows evaluation ISO, about 6.6 GiB");
    println!("  from {EVAL_FWLINK}");
    println!("  to   {}", destination.display());
    let code = runner
        .stream(&command)
        .map_err(|e| format!("cannot run the downloader: {e}"))?;

    let size = std::fs::metadata(&destination)
        .map(|m| m.len())
        .unwrap_or(0);
    if code != 0 || !plausible_iso(size) {
        let _ = std::fs::remove_file(&destination);
        return Err(manual_instructions(&destination));
    }
    Ok(destination)
}

/// Pick a downloader. `curl` ships with Windows 10 and later and with most
/// Linux distributions; `wget` covers the rest.
pub fn download_command(runner: &dyn Runner, url: &str, destination: &Path) -> Option<Cmd> {
    let out = destination.to_string_lossy().into_owned();
    if runner.which("curl").is_some() {
        return Some(Cmd::new("curl").args([
            "--location".to_owned(),
            "--fail".to_owned(),
            "--retry".to_owned(),
            "3".to_owned(),
            "--output".to_owned(),
            out,
            url.to_owned(),
        ]));
    }
    if runner.which("wget").is_some() {
        return Some(Cmd::new("wget").args(["--output-document".to_owned(), out, url.to_owned()]));
    }
    None
}

/// The ISO builder this repack needs.
///
/// Not the four-way search `packer_iso_tool` does: that list exists because
/// Packer resolves the tool itself and takes whichever it finds first. Here the
/// command line is ours, and only one of the four speaks it. This runs on a
/// Windows host only, which is where oscdimg is the one available anyway.
pub const ISO_BUILDER: &str = "oscdimg";

/// Where the boot images live inside Windows installation media, relative to
/// its root.
///
/// The prompting one is `efisys.bin`, next to the one used here. Two boot
/// images rather than one because the media is bootable on both firmwares, and
/// keeping the BIOS entry costs nothing.
pub const EFI_BOOT_IMAGE: &str = r"efi\microsoft\boot\efisys_noprompt.bin";
pub const BIOS_BOOT_IMAGE: &str = r"boot\etfsboot.com";

/// The boot images to make an ISO bootable with, absolute paths into a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootImages {
    /// `etfsboot.com`, when the media has one.
    pub bios: Option<PathBuf>,
    /// `efisys_noprompt.bin`, which is the point of the exercise.
    pub efi: PathBuf,
}

impl BootImages {
    /// Find them in an extracted media tree. The EFI image is required; a
    /// medium without the BIOS one is packed for UEFI alone.
    pub fn locate(tree: &Path) -> Result<Self, String> {
        let efi = tree.join(EFI_BOOT_IMAGE);
        if !efi.is_file() {
            return Err(format!(
                "this installation media has no {EFI_BOOT_IMAGE}, so the boot \
                 prompt cannot be removed from it. Every Windows 10 and 11 \
                 medium carries one next to efisys.bin; a medium without it is \
                 not one this build knows how to use."
            ));
        }
        let bios = tree.join(BIOS_BOOT_IMAGE);
        Ok(Self {
            bios: bios.is_file().then_some(bios),
            efi,
        })
    }

    /// The `-bootdata:` argument, which is one argument however many entries it
    /// carries.
    ///
    /// Documented form: `-bootdata:<count>#p<platform>,e,b<image>` per entry,
    /// entries separated by `#`, platform `0` for BIOS and `EF` for UEFI, `e`
    /// for "no emulation".
    pub fn bootdata_arg(&self) -> String {
        let mut entries = Vec::new();
        if let Some(bios) = &self.bios {
            entries.push(format!("p0,e,b{}", bios.display()));
        }
        entries.push(format!("pEF,e,b{}", self.efi.display()));
        format!("-bootdata:{}#{}", entries.len(), entries.join("#"))
    }
}

/// The `oscdimg` command that packs `tree` into `out`.
///
/// `-u2 -udfver102` is UDF only, which is what Windows installation media is:
/// it has no name-length limits to fall foul of, and every Windows since XP
/// reads it, `WinPE` included. `-m` lifts the size limit, since the media is well
/// past a CD's, and `-o` deduplicates identical files, which install media has
/// plenty of.
pub fn oscdimg_command(
    tool: &Path,
    tree: &Path,
    out: &Path,
    label: Option<&str>,
    boot: Option<&BootImages>,
) -> Cmd {
    let mut args = vec![
        "-m".to_owned(),
        "-o".to_owned(),
        "-u2".to_owned(),
        "-udfver102".to_owned(),
    ];
    if let Some(label) = label.map(str::trim).filter(|l| !l.is_empty()) {
        args.push(format!("-l{label}"));
    }
    if let Some(boot) = boot {
        args.push(boot.bootdata_arg());
    }
    args.push(tree.to_string_lossy().into_owned());
    args.push(out.to_string_lossy().into_owned());
    Cmd::new(tool.to_string_lossy()).args(args)
}

/// Mount an ISO read-only and report the drive letter and volume label.
///
/// Mounted rather than read with a library: Windows has the reader already, it
/// needs no elevation for an ISO, and an extra ISO9660 and UDF parser is not
/// something this crate should own.
///
/// The retry loop is not decoration. `Mount-DiskImage` returns before the
/// volume has a drive letter, so asking once answers "no letter" on a mount
/// that is perfectly fine half a second later.
pub fn mount_script(iso: &Path) -> String {
    format!(
        "$path = {path}\n\
         $image = Get-DiskImage -ImagePath $path\n\
         if (-not $image.Attached) {{\n  \
         $image = Mount-DiskImage -ImagePath $path -StorageType ISO -PassThru\n\
         }}\n\
         $letter = ''\n\
         $label = ''\n\
         foreach ($attempt in 1..30) {{\n  \
         $volume = Get-DiskImage -ImagePath $path | Get-Volume\n  \
         if ($volume -and $volume.DriveLetter) {{\n    \
         $letter = [string]$volume.DriveLetter\n    \
         $label = [string]$volume.FileSystemLabel\n    \
         break\n  \
         }}\n  \
         Start-Sleep -Milliseconds 500\n\
         }}\n\
         Write-Output \"DRIVE=$letter\"\n\
         Write-Output \"LABEL=$label\"\n",
        path = ps_quote(iso)
    )
}

/// Unmount it again, tolerating it not being mounted.
pub fn dismount_script(iso: &Path) -> String {
    format!(
        // In a try rather than with `-ErrorAction SilentlyContinue`: a
        // suppressed error still leaves `powershell.exe` exiting 1, and this is
        // called on the way out of a failure, where a second misleading message
        // is the last thing wanted.
        "$path = {path}\n\
         $image = $null\n\
         try {{ $image = Get-DiskImage -ImagePath $path -ErrorAction Stop }} catch {{ }}\n\
         if ($image -and $image.Attached) {{\n  \
         Dismount-DiskImage -ImagePath $path | Out-Null\n\
         }}\n",
        path = ps_quote(iso)
    )
}

/// What one media repack costs in disk space while it runs.
///
/// The extraction tree is the size of the media and so is the ISO built from
/// it, and they exist at the same time; the tree goes on the way out, so the
/// lasting cost is one copy. Reported rather than checked: the budget belongs in
/// the message of whatever ran out of room.
pub fn repack_budget_bytes(iso_bytes: u64) -> u64 {
    iso_bytes.saturating_mul(2)
}

/// What a prompt-free copy was repacked from.
///
/// The original download's size and checksum, written beside the copy. Without
/// it the copy is tied to nothing, and the only question ever asked of a cached
/// one was whether it was large enough to be an ISO: a re-downloaded original at
/// the next Windows revision would leave a repack of the previous one in place,
/// and the install after that would install the previous one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMark {
    #[serde(default)]
    pub bytes: u64,
    /// `crc32:xxxxxxxx`, the same form the manifest records.
    #[serde(default)]
    pub checksum: String,
}

impl SourceMark {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("malformed media record: {e}"))
    }

    /// The mark of a file on disk. A CRC-32 reads at several gigabytes a
    /// second, so this is a few seconds against a build's quarter of an hour.
    pub fn of(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::metadata(path)
            .map_err(|e| format!("cannot stat {}: {e}", path.display()))?
            .len();
        let checksum = crate::store::hash::checksum_file(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Ok(Self { bytes, checksum })
    }
}

/// Read the record beside a cached copy, if there is a readable one.
pub fn read_source_mark(path: &Path) -> Option<SourceMark> {
    SourceMark::from_json(&std::fs::read_to_string(path).ok()?).ok()
}

/// The name of that record, which is where [`Store::windows_iso_noprompt_source`]
/// puts it and what a listing of the media directory recognizes it by.
pub const SOURCE_MARK_FILE: &str = "windows11-enterprise-eval-noprompt.source.json";

/// What a file in the media directory is, when it is not media.
///
/// The directory holds the download, the prompt-free copy, and the record tying
/// the second to the first, and `vm purge windows --iso` takes all three: a
/// record that outlived its copy would claim a provenance for a file that is no
/// longer there (departure 43). So a listing shows all three rather than showing
/// two and deleting three, and says which of them is not an ISO.
pub fn media_note(name: &str) -> Option<&'static str> {
    (name == SOURCE_MARK_FILE)
        .then_some("the record of which download the prompt-free copy was made from")
}

/// The size of a file, or `None` if it is not there.
fn file_bytes(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|meta| meta.len())
}

/// What a cached prompt-free copy of the media is worth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cached {
    /// It is a copy of the download that is on disk.
    Reuse,
    /// Use it, and say why nothing checked it against an original.
    ReuseUnchecked(String),
    /// Make it again, for this reason.
    Repack(String),
}

/// Decide what to do with a cached prompt-free copy.
///
/// `source_checksum` is asked for only once the sizes agree, because it reads
/// the whole 6.6 GB download: a size that disagrees has already answered the
/// question, and a build should not pay for an answer it has.
///
/// The missing-original case is deliberate. The copy is what the install boots,
/// so a copy whose original has been deleted is used rather than re-fetched:
/// downloading 6.6 GB to remake a file that is already there is the worst of the
/// available outcomes. Saying that nothing checked it is the honest half of
/// that.
pub fn cached_noprompt(
    copy_bytes: Option<u64>,
    mark: Option<&SourceMark>,
    source_bytes: Option<u64>,
    source_checksum: &dyn Fn() -> Result<String, String>,
) -> Cached {
    match copy_bytes {
        None => return Cached::Repack("there is no prompt-free copy of the media yet".to_owned()),
        Some(bytes) if !plausible_iso(bytes) => {
            return Cached::Repack(format!(
                "the cached prompt-free media is only {}, which is too small to be an ISO",
                format_bytes(bytes)
            ));
        }
        Some(_) => {}
    }
    let Some(mark) = mark else {
        return match source_bytes {
            Some(_) => Cached::Repack(
                "nothing records which download the cached prompt-free media was made from"
                    .to_owned(),
            ),
            None => Cached::ReuseUnchecked(
                "nothing records which download it was made from, and the original is not \
                 on disk to check it against"
                    .to_owned(),
            ),
        };
    };
    let Some(bytes) = source_bytes else {
        return Cached::ReuseUnchecked(format!(
            "the original download is not on disk, so it was not checked against the {} \
             this copy was made from",
            format_bytes(mark.bytes)
        ));
    };
    if bytes != mark.bytes {
        return Cached::Repack(format!(
            "the download is {} and the cached prompt-free media was made from one of {}",
            format_bytes(bytes),
            format_bytes(mark.bytes)
        ));
    }
    match source_checksum() {
        Ok(checksum) if checksum == mark.checksum => Cached::Reuse,
        Ok(checksum) => Cached::Repack(format!(
            "the download is {checksum} and the cached prompt-free media was made from {}",
            mark.checksum
        )),
        // Nothing can be repacked out of a file that cannot be read either, so
        // the copy already on disk is the better of two poor answers.
        Err(e) => Cached::ReuseUnchecked(format!("the original could not be checked ({e})")),
    }
}

/// The media the native install boots, and the download it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallMedia {
    /// The prompt-free copy, which is the disc the build attaches.
    pub boot: PathBuf,
    /// The original download, which the manifest records as the image's source.
    /// It may not be on disk any more: what boots is the copy.
    pub source: PathBuf,
}

/// Get the media the native install boots: the prompt-free copy, repacking it
/// from the download and fetching that download only if there is a repack to do.
///
/// Windows installation media carries two UEFI boot images:
/// `efi\microsoft\boot\efisys.bin`, which prints "Press any key to boot from CD
/// or DVD" and waits for one, and `efisys_noprompt.bin`, which boots the
/// installer immediately. Repacking with the second one is what makes an
/// unattended install unattended from the first second: no prompt means no
/// keypress, no keyboard injection, and none of the QMP machinery the QEMU path
/// needs (amendment decision 16).
///
/// The result is cached beside the original and is inventory like it: `vm
/// status` lists both, and `vm purge windows --iso` deletes both along with the
/// record of what the copy was made from.
pub fn ensure_install_media(
    runner: &dyn Runner,
    store: &Store,
    build_dir: &Path,
) -> Result<InstallMedia, String> {
    let source = store.windows_iso();
    let copy = store.windows_iso_noprompt();
    let mark = read_source_mark(&store.windows_iso_noprompt_source());
    let checked = || {
        // Said rather than done silently: it reads 6.6 GB, which is seconds
        // rather than nothing.
        println!("checking the cached prompt-free media against the download it came from");
        crate::store::hash::checksum_file(&source)
            .map_err(|e| format!("cannot read {}: {e}", source.display()))
    };
    let decision = cached_noprompt(
        file_bytes(&copy),
        mark.as_ref(),
        file_bytes(&source),
        &checked,
    );

    match decision {
        Cached::Repack(why) => {
            println!("repacking the installation media: {why}");
        }
        reuse => {
            println!(
                "using the cached prompt-free media at {} ({})",
                copy.display(),
                format_bytes(file_bytes(&copy).unwrap_or(0))
            );
            if let Cached::ReuseUnchecked(why) = reuse {
                println!("  {why}");
            }
            return Ok(InstallMedia { boot: copy, source });
        }
    }

    let source = ensure_iso(runner, store)?;
    let boot = repack_noprompt(runner, store, &source, build_dir)?;
    Ok(InstallMedia { boot, source })
}

/// Delete the prompt-free copy and the record beside it.
///
/// It is 6.6 GiB of derived data: the download with one boot image swapped, so
/// a repack makes it again in minutes against an image build that costs an
/// hour. Keeping it bought that saving with twice the disk of the download it
/// came from, for a rebuild that happens when a template changes or an
/// evaluation expires. So a build that succeeded takes it back out, and what
/// stays is the one file that cannot be made again from anything on this host.
///
/// The record goes first for the same reason [`repack_noprompt`] writes it
/// last: a record that outlived its copy claims a provenance for a file that is
/// not there. The other order is safe too, because [`cached_noprompt`] repacks
/// when either half is missing, but only one of the two can be stated in one
/// sentence.
pub fn discard_install_media(store: &Store) {
    let copy = store.windows_iso_noprompt();
    let bytes = file_bytes(&copy);
    let _ = std::fs::remove_file(store.windows_iso_noprompt_source());
    match std::fs::remove_file(&copy) {
        Ok(()) => println!(
            "removed the prompt-free installation media, freeing {}",
            format_bytes(bytes.unwrap_or(0))
        ),
        // Nothing to remove is the ordinary case on a second build.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => println!("warning: cannot remove {}: {e}", copy.display()),
    }
}

/// What a build that did not finish is leaving behind, if anything.
///
/// A failed build keeps the repack, because the retry is the one occasion when
/// remaking it is a cost worth avoiding, and says so rather than holding
/// several gigabytes quietly.
pub fn kept_install_media(store: &Store) -> Option<String> {
    let copy = store.windows_iso_noprompt();
    let bytes = file_bytes(&copy)?;
    Some(format!(
        "keeping the prompt-free installation media at {} ({}), which the next \
         attempt reuses. `cargo xtask vm purge windows --iso` removes it along \
         with the download.",
        copy.display(),
        format_bytes(bytes)
    ))
}

/// Make the prompt-free copy, and record what it was made from.
fn repack_noprompt(
    runner: &dyn Runner,
    store: &Store,
    source: &Path,
    build_dir: &Path,
) -> Result<PathBuf, String> {
    let destination = store.windows_iso_noprompt();
    let record = store.windows_iso_noprompt_source();
    // The record goes first, so a repack that fails partway cannot leave one
    // describing a file that is no longer the file beside it.
    let _ = std::fs::remove_file(&record);

    let host = crate::provider::target::HostOs::current();
    let tool = crate::host::facts::resolve_tool(runner, ISO_BUILDER, host).ok_or_else(|| {
        format!(
            "{ISO_BUILDER} is not available, and it is what removes the boot prompt \
             from the installation media. `cargo xtask vm setup` installs it, and \
             `cargo xtask vm doctor` reports where it found it."
        )
    })?;

    let source_bytes = file_bytes(source).unwrap_or(0);
    let tree = build_dir.join("media-tree");
    let staged = build_dir.join("noprompt.iso");
    println!(
        "repacking the installation media without its boot prompt, using {}",
        tool.display()
    );
    println!(
        "  this copies the media out and back, so it needs about {} free under {}",
        format_bytes(repack_budget_bytes(source_bytes)),
        build_dir.display()
    );

    let _ = std::fs::remove_dir_all(&tree);
    let _ = std::fs::remove_file(&staged);
    std::fs::create_dir_all(&tree).map_err(|e| {
        format!(
            "cannot create {}: {e}. The extraction needs about {} free there.",
            tree.display(),
            format_bytes(repack_budget_bytes(source_bytes))
        )
    })?;

    // Packing and putting the result where it belongs are one operation with
    // one cleanup site, because the staged ISO is another copy of the media and
    // a move that failed leaves it exactly where a failed pack does.
    let packed = pack_noprompt(runner, &tool, source, &tree, &staged, build_dir)
        .and_then(|()| install_packed(&staged, &destination));
    // The tree is the size of the media, and it goes on every path out of here
    // rather than only the ones that reached oscdimg: media that cannot be
    // mounted and media with no noprompt boot image both return above that
    // point, and both are failures somebody is about to retry.
    let _ = std::fs::remove_dir_all(&tree);
    if let Err(e) = packed {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }

    let bytes = file_bytes(&destination).unwrap_or(0);
    if !plausible_iso(bytes) {
        let _ = std::fs::remove_file(&destination);
        return Err(format!(
            "the repacked media came out at {}, which is too small to be Windows \
             installation media. Nothing is cached, so running this again is safe.",
            format_bytes(bytes)
        ));
    }
    println!(
        "  prompt-free media at {} ({})",
        destination.display(),
        format_bytes(bytes)
    );

    // Written last, because it is the claim that the copy beside it came from
    // this download. A repack with no record is repacked again next time, which
    // costs a minute; a record for a copy that was never finished would cost an
    // install.
    match SourceMark::of(source) {
        Ok(mark) => {
            if let Err(e) = std::fs::write(&record, mark.to_json()) {
                println!("warning: cannot record what the media was made from ({e})");
            }
        }
        Err(e) => println!("warning: cannot read the download to record it ({e})"),
    }
    Ok(destination)
}

/// Put the packed ISO in the media directory, making the directory if it is not
/// there yet.
///
/// Part of the same fallible step as the pack, so that the staged ISO has one
/// place to be deleted from: it is 6.6 GB, and a move that could not be made is
/// no more worth keeping than a pack that could not be finished.
fn install_packed(staged: &Path, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    move_file(staged, destination)
}

/// Extract the media, find its boot images, and pack the copy.
///
/// Separate from [`repack_noprompt`] so that the transient tree has one place to
/// be deleted from, whichever of these steps failed.
fn pack_noprompt(
    runner: &dyn Runner,
    tool: &Path,
    source: &Path,
    tree: &Path,
    staged: &Path,
    build_dir: &Path,
) -> Result<(), String> {
    let label = extract_media(runner, source, tree)?;
    let boot = BootImages::locate(tree)?;

    println!(
        "  packing {} into an ISO that boots straight in",
        tree.display()
    );
    let command = oscdimg_command(tool, tree, staged, label.as_deref(), Some(&boot));
    let code = runner
        .stream(&command)
        .map_err(|e| format!("cannot run {}: {e}", tool.display()))?;
    if code != 0 {
        return Err(format!(
            "{ISO_BUILDER} exited {code} while repacking the installation media. \
             It needs about {} free under {} for the ISO it writes.",
            format_bytes(file_bytes(source).unwrap_or(0)),
            build_dir.display()
        ));
    }
    Ok(())
}

/// Mount the media, copy its tree out, and unmount it whatever happened.
///
/// Returns the volume label, so the repack can carry it over: nothing in Setup
/// depends on it, and an ISO that reports itself as something else is a
/// needless difference from the media it was made from.
fn extract_media(
    runner: &dyn Runner,
    source: &Path,
    tree: &Path,
) -> Result<Option<String>, String> {
    let mounted = run_script(runner, &mount_script(source));
    let outcome = match &mounted {
        Err(e) => Err(format!("cannot mount {}: {e}", source.display())),
        Ok(stdout) => copy_media_out(source, tree, stdout),
    };

    // Always, and that includes a mount script that failed. It is one
    // `powershell.exe` with `$ErrorActionPreference = 'Stop'` in front of it, so
    // an error record anywhere after `Mount-DiskImage` ends the script with the
    // image attached and holding a drive letter; the retry loop that waits for
    // the letter is on that side of the mount. `dismount_script` tolerates an
    // image that is not mounted, so asking unconditionally costs nothing.
    if let Err(e) = run_script(runner, &dismount_script(source)) {
        println!("warning: {} is still mounted ({e})", source.display());
    }
    outcome
}

/// Copy the mounted media out, given what the mount script said about it.
fn copy_media_out(source: &Path, tree: &Path, mounted: &str) -> Result<Option<String>, String> {
    let label = crate::util::marker(mounted, "LABEL=");
    let Some(letter) = crate::util::marker(mounted, "DRIVE=") else {
        return Err(format!(
            "{} mounted without being given a drive letter, so there is nothing to \
             copy from. `Get-DiskImage -ImagePath <path> | Get-Volume` shows what \
             Windows made of it.",
            source.display()
        ));
    };
    let root = PathBuf::from(format!("{letter}:\\"));
    println!("  copying the media out of {}", root.display());
    let started = std::time::Instant::now();
    let (files, bytes) = copy_tree(&root, tree).map_err(|e| {
        format!(
            "cannot copy the installation media out of {}: {e}. The extraction \
             needs about the media's own size free under {}.",
            root.display(),
            tree.display()
        )
    })?;
    println!(
        "  copied {} ({}) in {}",
        crate::util::count(files, "file"),
        format_bytes(bytes),
        crate::util::format_duration(started.elapsed())
    );
    Ok(label)
}

/// Run a `PowerShell` script and hand back its stdout.
fn run_script(runner: &dyn Runner, script: &str) -> Result<String, String> {
    let out = runner
        .capture(&powershell(script))
        .map_err(|e| format!("cannot run powershell.exe: {e}"))?;
    if out.success() {
        Ok(out.stdout)
    } else {
        Err(format!("exit {:?}: {}", out.code, out.stderr.trim()))
    }
}

/// Copy a directory tree, clearing the read-only attribute as it goes.
///
/// Recursive by hand rather than through a copy tool: `robocopy` reports
/// success with an exit code that has to be interpreted, and `Copy-Item
/// -Recurse` of a mounted ISO is slower than this by a wide margin. The
/// attribute matters because every file on a mounted ISO is read-only, Windows
/// copies that attribute along, and `remove_dir_all` over read-only files fails.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<(usize, u64)> {
    let mut files = 0;
    let mut bytes = 0;
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            let (sub_files, sub_bytes) = copy_tree(&entry.path(), &target)?;
            files += sub_files;
            bytes += sub_bytes;
        } else if kind.is_file() {
            bytes += std::fs::copy(entry.path(), &target)?;
            files += 1;
            make_writable(&target);
        }
    }
    Ok((files, bytes))
}

/// Clear the read-only attribute that came along with a file.
///
/// Windows only, because that is where it matters and where it is free: there
/// every file on a mounted ISO is read-only, `CopyFileEx` brings the attribute
/// with it, and `remove_dir_all` over read-only files fails, which would leave
/// the transient tree undeletable. On Unix the containing directory's
/// permissions decide whether a file can be unlinked, so clearing the bit there
/// would widen the mode for no gain.
#[cfg(windows)]
fn make_writable(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut permissions = meta.permissions();
        if permissions.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(path, permissions);
        }
    }
}

#[cfg(not(windows))]
fn make_writable(_path: &Path) {}

/// Rename, falling back to a copy across volumes.
fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(to);
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map(|_| ()).map_err(|e| {
        // A copy that stopped partway leaves a piece of an ISO where a whole one
        // belongs, and nothing downstream would ever call it anything but media.
        let _ = std::fs::remove_file(to);
        format!("cannot move {} to {}: {e}", from.display(), to.display())
    })?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::CommandOutput;
    use crate::runner::fake::FakeRunner;

    #[test]
    fn an_error_page_is_not_mistaken_for_an_iso() {
        assert!(!plausible_iso(0));
        assert!(!plausible_iso(4096));
        assert!(!plausible_iso(MIN_PLAUSIBLE_ISO_BYTES - 1));
        assert!(plausible_iso(MIN_PLAUSIBLE_ISO_BYTES));
        assert!(plausible_iso(7_092_807_680));
    }

    #[test]
    fn the_manual_instructions_name_the_page_and_the_exact_path() {
        let text = manual_instructions(Path::new(r"C:\vm\iso\windows11-enterprise-eval.iso"));
        assert!(text.contains(EVAL_PAGE), "{text}");
        assert!(
            text.contains(r"C:\vm\iso\windows11-enterprise-eval.iso"),
            "{text}"
        );
        assert!(text.contains("build-image windows"), "{text}");
    }

    #[test]
    fn curl_is_preferred_and_follows_redirects() {
        let runner = FakeRunner::new().with_tool("curl", "/usr/bin/curl");
        let cmd = download_command(&runner, EVAL_FWLINK, Path::new("/vm/iso/win.iso"))
            .expect("curl is available");
        assert_eq!(cmd.program, "curl");
        assert!(
            cmd.args.contains(&"--location".to_owned()),
            "{:?}",
            cmd.args
        );
        assert!(cmd.args.contains(&"--fail".to_owned()), "{:?}", cmd.args);
        assert_eq!(cmd.args.last().map(String::as_str), Some(EVAL_FWLINK));
    }

    #[test]
    fn wget_is_the_fallback_and_no_downloader_is_not_a_crash() {
        let runner = FakeRunner::new().with_tool("wget", "/usr/bin/wget");
        assert_eq!(
            download_command(&runner, EVAL_FWLINK, Path::new("/vm/iso/win.iso")).map(|c| c.program),
            Some("wget".to_owned())
        );
        let bare = FakeRunner::new();
        assert!(download_command(&bare, EVAL_FWLINK, Path::new("/vm/iso/win.iso")).is_none());
    }

    fn boot_images() -> BootImages {
        BootImages {
            bios: Some(PathBuf::from(r"C:\t\boot\etfsboot.com")),
            efi: PathBuf::from(r"C:\t\efi\microsoft\boot\efisys_noprompt.bin"),
        }
    }

    #[test]
    fn the_repack_command_is_the_documented_one_for_windows_media() {
        let cmd = oscdimg_command(
            Path::new(r"C:\tools\oscdimg.exe"),
            Path::new(r"C:\t"),
            Path::new(r"C:\out.iso"),
            Some("CCCOMA_X64FRE_EN-US_DV9"),
            Some(&boot_images()),
        );
        assert_eq!(cmd.program, r"C:\tools\oscdimg.exe");
        // UDF only, no size limit, deduplicated: what installation media is.
        for flag in ["-m", "-o", "-u2", "-udfver102"] {
            assert!(cmd.args.contains(&flag.to_owned()), "{:?}", cmd.args);
        }
        assert!(
            cmd.args.contains(&"-lCCCOMA_X64FRE_EN-US_DV9".to_owned()),
            "{:?}",
            cmd.args
        );
        // The tree and the output go last, in that order.
        assert_eq!(
            &cmd.args[cmd.args.len() - 2..],
            [r"C:\t".to_owned(), r"C:\out.iso".to_owned()]
        );
        // One argument for both boot images, no spaces in it: a shell would
        // split it and there is no shell here.
        let bootdata = cmd
            .args
            .iter()
            .find(|a| a.starts_with("-bootdata:"))
            .expect("the boot images are what this repack is for");
        assert_eq!(
            bootdata,
            r"-bootdata:2#p0,e,bC:\t\boot\etfsboot.com#pEF,e,bC:\t\efi\microsoft\boot\efisys_noprompt.bin"
        );
        assert!(!bootdata.contains(' '), "{bootdata}");
        // The prompting image must not be the one packed: answering the prompt
        // is exactly what this avoids having to do.
        assert!(!bootdata.contains("efisys.bin"), "{bootdata}");
    }

    #[test]
    fn media_without_a_bios_boot_image_is_packed_for_uefi_alone() {
        let efi_only = BootImages {
            bios: None,
            efi: PathBuf::from(r"C:\t\efi\microsoft\boot\efisys_noprompt.bin"),
        };
        assert_eq!(
            efi_only.bootdata_arg(),
            r"-bootdata:1#pEF,e,bC:\t\efi\microsoft\boot\efisys_noprompt.bin"
        );
    }

    #[test]
    fn an_unlabelled_or_unbootable_pack_omits_those_flags_rather_than_passing_empty_ones() {
        let cmd = oscdimg_command(
            Path::new("oscdimg"),
            Path::new("/t"),
            Path::new("/out.iso"),
            Some("   "),
            None,
        );
        assert!(
            !cmd.args.iter().any(|a| a.starts_with("-l")),
            "{:?}",
            cmd.args
        );
        assert!(
            !cmd.args.iter().any(|a| a.starts_with("-bootdata")),
            "{:?}",
            cmd.args
        );
    }

    #[test]
    fn the_efi_boot_image_is_required_and_the_bios_one_is_not() {
        let dir = std::env::temp_dir().join("sunlit_xtask_boot_images");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("efi/microsoft/boot")).expect("temp tree");
        let missing = BootImages::locate(&dir).expect_err("no boot image yet");
        assert!(missing.contains("efisys_noprompt.bin"), "{missing}");

        std::fs::write(dir.join("efi/microsoft/boot/efisys_noprompt.bin"), b"x").expect("write");
        let found = BootImages::locate(&dir).expect("the EFI image is there");
        assert_eq!(found.bios, None);

        std::fs::create_dir_all(dir.join("boot")).expect("temp tree");
        std::fs::write(dir.join("boot/etfsboot.com"), b"x").expect("write");
        let both = BootImages::locate(&dir).expect("both images are there");
        assert!(both.bios.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mounting_asks_again_until_the_volume_has_a_letter() {
        let script = mount_script(Path::new(r"C:\vm\iso\win.iso"));
        assert!(script.contains(r"'C:\vm\iso\win.iso'"), "{script}");
        assert!(script.contains("-StorageType ISO"), "{script}");
        // Mounted only if it is not already, because mounting a mounted image
        // is an error rather than a no-op.
        assert!(script.contains("if (-not $image.Attached)"), "{script}");
        // The letter arrives after the mount returns, so asking once is a
        // guess.
        assert!(script.contains("Start-Sleep"), "{script}");
        assert!(script.contains("DRIVE=$letter"), "{script}");
        assert!(script.contains("LABEL=$label"), "{script}");
        // `-f` is a string operator, not a parameter of Write-Output.
        assert!(!script.contains("\" -f "), "{script}");

        let dismount = dismount_script(Path::new(r"C:\vm\iso\win.iso"));
        assert!(dismount.contains("Dismount-DiskImage"), "{dismount}");
        assert!(dismount.contains("$image.Attached"), "{dismount}");
    }

    #[test]
    fn the_repack_budget_is_two_copies_of_the_media() {
        assert_eq!(repack_budget_bytes(7_092_807_680), 14_185_615_360);
        assert_eq!(repack_budget_bytes(0), 0);
    }

    #[test]
    fn a_copied_tree_keeps_its_shape_and_loses_the_read_only_attribute() {
        // Every file on a mounted ISO is read-only, Windows copies the
        // attribute along, and `remove_dir_all` over read-only files fails:
        // the transient tree would then be undeletable.
        let dir = std::env::temp_dir().join("sunlit_xtask_copy_tree");
        let _ = std::fs::remove_dir_all(&dir);
        let from = dir.join("from");
        std::fs::create_dir_all(from.join("efi/microsoft/boot")).expect("temp tree");
        std::fs::write(from.join("setup.exe"), b"setup").expect("write");
        std::fs::write(from.join("efi/microsoft/boot/efisys_noprompt.bin"), b"boot")
            .expect("write");
        let mut readonly = std::fs::metadata(from.join("setup.exe"))
            .expect("stat")
            .permissions();
        readonly.set_readonly(true);
        std::fs::set_permissions(from.join("setup.exe"), readonly).expect("set read-only");

        let to = dir.join("to");
        let (files, bytes) = copy_tree(&from, &to).expect("copy");
        assert_eq!(files, 2);
        assert_eq!(bytes, 9);
        assert!(to.join("efi/microsoft/boot/efisys_noprompt.bin").is_file());
        #[cfg(windows)]
        assert!(
            !std::fs::metadata(to.join("setup.exe"))
                .expect("stat")
                .permissions()
                .readonly()
        );
        // Which is the property that matters: the tree can be removed again.
        std::fs::remove_dir_all(&dir).expect("the transient tree has to be deletable");
    }

    fn mark(bytes: u64, checksum: &str) -> SourceMark {
        SourceMark {
            bytes,
            checksum: checksum.to_owned(),
        }
    }

    #[test]
    fn a_cached_repack_is_reused_only_when_it_matches_the_download_it_came_from() {
        let iso = 7_092_807_680;
        let same = || Ok("crc32:11112222".to_owned());
        // The copy, the record beside it, and the download all agree.
        assert_eq!(
            cached_noprompt(
                Some(iso),
                Some(&mark(iso, "crc32:11112222")),
                Some(iso),
                &same
            ),
            Cached::Reuse
        );

        // A download of the same size with different contents is a different
        // revision, and an install from the old repack would install it.
        let repack = cached_noprompt(
            Some(iso),
            Some(&mark(iso, "crc32:33334444")),
            Some(iso),
            &same,
        );
        assert!(
            matches!(&repack, Cached::Repack(why) if why.contains("crc32:11112222")),
            "{repack:?}"
        );

        // A download of a different size does not need reading to decide.
        let cheap = std::cell::Cell::new(0);
        let counted = || {
            cheap.set(cheap.get() + 1);
            Ok("crc32:11112222".to_owned())
        };
        let resized = cached_noprompt(
            Some(iso),
            Some(&mark(iso, "crc32:11112222")),
            Some(iso + 4096),
            &counted,
        );
        assert!(matches!(resized, Cached::Repack(_)), "{resized:?}");
        assert_eq!(
            cheap.get(),
            0,
            "a size that already decided it must not cost a pass over 6.6 GB"
        );
    }

    #[test]
    fn a_copy_with_no_original_behind_it_is_used_rather_than_re_downloaded() {
        // The copy is what the install boots. Fetching 6.6 GB again to remake a
        // file that is already there is the worst of the outcomes, so it is used
        // and the report says nothing checked it.
        let iso = 7_092_807_680;
        let unread = || Err("no such file".to_owned());
        let gone = cached_noprompt(Some(iso), Some(&mark(iso, "crc32:1")), None, &unread);
        assert!(
            matches!(&gone, Cached::ReuseUnchecked(why) if why.contains("not on disk")),
            "{gone:?}"
        );

        // Nor when neither the original nor a record of it is there.
        let bare = cached_noprompt(Some(iso), None, None, &unread);
        assert!(matches!(bare, Cached::ReuseUnchecked(_)), "{bare:?}");

        // But with the original there and nothing recording the copy's origin,
        // the copy is remade: that is the state every cache from before the
        // record existed is in, and it costs a repack once.
        let unrecorded = cached_noprompt(Some(iso), None, Some(iso), &unread);
        assert!(matches!(unrecorded, Cached::Repack(_)), "{unrecorded:?}");
    }

    #[test]
    fn nothing_cached_or_too_small_to_be_an_iso_is_repacked() {
        let never = || Err("not asked".to_owned());
        assert!(matches!(
            cached_noprompt(None, None, Some(7_092_807_680), &never),
            Cached::Repack(_)
        ));
        let small = cached_noprompt(Some(4096), None, Some(7_092_807_680), &never);
        assert!(
            matches!(&small, Cached::Repack(why) if why.contains("too small")),
            "{small:?}"
        );
    }

    #[test]
    fn the_record_of_what_a_copy_was_made_from_round_trips() {
        let written = mark(7_092_807_680, "crc32:deadbeef");
        let read = SourceMark::from_json(&written.to_json()).expect("round trip");
        assert_eq!(read, written);
        assert!(SourceMark::from_json("not json").is_err());
        // A record that cannot be read is no record, not a crash.
        let dir = std::env::temp_dir().join("sunlit_xtask_media_mark");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        assert_eq!(read_source_mark(&dir.join("missing.json")), None);
        std::fs::write(dir.join("broken.json"), b"{").expect("write");
        assert_eq!(read_source_mark(&dir.join("broken.json")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A build that produced an image takes the repack back out: it is the
    /// largest thing in the store that can simply be made again.
    #[test]
    fn a_finished_build_leaves_only_the_download_behind() {
        let dir = std::env::temp_dir().join("sunlit_xtask_media_discard");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::new(dir.join("store"));
        std::fs::create_dir_all(store.iso_dir()).expect("temp tree");
        let download = store.windows_iso();
        let copy = store.windows_iso_noprompt();
        let record = store.windows_iso_noprompt_source();
        for path in [&download, &copy, &record] {
            std::fs::write(path, b"pretend").expect("write");
        }

        // A failed build says what it is keeping, and keeps it.
        let note = kept_install_media(&store).expect("something is being kept");
        assert!(
            note.contains("windows11-enterprise-eval-noprompt.iso"),
            "{note}"
        );
        assert!(note.contains("purge windows --iso"), "{note}");
        assert!(copy.is_file());

        discard_install_media(&store);
        assert!(!copy.exists(), "the prompt-free copy is derived data");
        assert!(
            !record.exists(),
            "and so is the record of where it came from"
        );
        assert!(
            download.is_file(),
            "the download is the one file that cannot be made again from this host"
        );

        // Nothing left to remove is the ordinary case on the next build, and
        // nothing to report either.
        discard_install_media(&store);
        assert_eq!(kept_install_media(&store), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The mount script is one `powershell.exe` with `$ErrorActionPreference =
    /// 'Stop'` in front of it, so an error record after `Mount-DiskImage` ends
    /// it with the image attached and holding a drive letter. And the extraction
    /// tree is the size of the media, so leaving it behind costs 6.6 GB.
    #[test]
    fn a_repack_that_failed_dismounts_the_media_and_leaves_no_tree_behind() {
        let dir = std::env::temp_dir().join("sunlit_xtask_repack_cleanup");
        let _ = std::fs::remove_dir_all(&dir);
        let build_dir = dir.join("build");
        std::fs::create_dir_all(&build_dir).expect("temp tree");
        let store = Store::new(dir.join("store"));
        let source = dir.join("windows.iso");
        std::fs::write(&source, b"not really an iso").expect("write");

        let runner = FakeRunner::new()
            .with_tool(ISO_BUILDER, "/tools/oscdimg")
            .on(
                "Mount-DiskImage",
                CommandOutput::failed(1, "access is denied"),
            )
            .on("Dismount-DiskImage", CommandOutput::ok(String::new()));

        let error = repack_noprompt(&runner, &store, &source, &build_dir)
            .expect_err("the mount failed, so the repack did");
        assert!(error.contains("cannot mount"), "{error}");
        assert!(
            !build_dir.join("media-tree").exists(),
            "the extraction tree is transient on every path out"
        );
        assert!(!build_dir.join("noprompt.iso").exists(), "{error}");
        // And the image was let go of, whether or not the script that mounted it
        // got as far as saying so.
        let dismount = crate::runner::powershell(&dismount_script(&source));
        assert!(
            runner.calls().contains(&dismount.display()),
            "{:?}",
            runner.calls()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_packed_iso_that_cannot_be_put_in_place_is_the_cleanup_sites_business() {
        // Moving the staged ISO into the media directory is part of the same
        // fallible step as packing it, because the file is another copy of the
        // media: the caller deletes it once, for either failure, and this half
        // therefore leaves it where the caller can find it.
        let dir = std::env::temp_dir().join("sunlit_xtask_install_packed");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp tree");
        let staged = dir.join("noprompt.iso");
        std::fs::write(&staged, b"packed").expect("write");

        // A parent that is a file is the portable way for the directory not to
        // be makeable.
        let blocked = dir.join("blocked");
        std::fs::write(&blocked, b"not a directory").expect("write");
        let error = install_packed(&staged, &blocked.join("windows.iso"))
            .expect_err("the media directory cannot be created");
        assert!(error.contains("cannot create"), "{error}");
        assert!(staged.is_file(), "the staged ISO is the caller's to delete");

        // And on the way it is meant to go, the destination directory is made
        // and the staged copy does not survive as a second one.
        let destination = dir.join("iso").join("windows.iso");
        install_packed(&staged, &destination).expect("the move");
        assert!(destination.is_file());
        assert!(!staged.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pinned_link_is_the_fwlink_rather_than_the_build_specific_one() {
        // The `prss` URL embeds a build number and dies at the next revision.
        assert!(EVAL_FWLINK.contains("go.microsoft.com/fwlink"));
        assert!(!EVAL_FWLINK.contains("prss.microsoft.com"));
    }
}
