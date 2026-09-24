//! `cargo xtask manifests`: the Scoop manifest, the Homebrew cask and the
//! Homebrew formula for one release, written from the release's own archives.
//!
//! Every file name comes from the bundle's naming functions, so a rename there
//! cannot leave a manifest pointing at an asset the release does not have, and
//! every hash is computed from the file rather than read from the GitHub API,
//! so the command runs the same against `gh release download` on a laptop as it
//! does in the workflow. The output is laid out as the two repositories are:
//! `bucket/` for `sunlit-earth/scoop-bucket`, `Casks/` and `Formula/` for
//! `sunlit-earth/homebrew-tap`.
//!
//! The Ruby is plain string assembly. Whether Homebrew accepts it is what
//! `brew style` and `brew audit` in `package-managers.yml` check, and nothing
//! here reimplements them.

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::commands::bundle::{self, APP_DIR, Arch, PACKAGE, Platform};

/// The repository whose releases the manifests download from.
pub const REPOSITORY: &str = "https://github.com/sunlit-earth/sunlit-earth";

/// Where each manifest goes under `--out`, which is where it lives in its
/// repository.
pub const SCOOP_MANIFEST: &str = "bucket/sunlit-earth.json";
pub const CASK: &str = "Casks/sunlit-earth.rb";
pub const FORMULA: &str = "Formula/sunlit-earth.rb";

const DESCRIPTION: &str = "View of Earth as seen from space as your wallpaper";
const LICENSE: &str = "GPL-3.0-or-later";
const APP_NAME: &str = "Sunlit Earth";

/// What Scoop prints after an install: `scoop update` skips an app while any
/// process runs from its directory, which the tray app always is.
const SCOOP_NOTES: &str = "Quit Sunlit Earth from its tray icon before `scoop update`, which skips an app that is running.";

/// `CFBundleIdentifier` in `assets/macos/Info.plist`, which `uninstall quit:`
/// asks to quit.
const BUNDLE_ID: &str = "earth.sunlit.SunlitEarth";

/// Where the macOS app keeps its configuration, caches and wallpapers.
const MACOS_DATA_DIR: &str = "~/Library/Application Support/SunlitEarth";

/// The Homebrew tokens a cask's `url` and `app` are templated with.
const VERSION_TOKEN: &str = "#{version}";
const ARCH_TOKEN: &str = "#{arch}";

/// Scoop's token in `autoupdate`.
const SCOOP_VERSION_TOKEN: &str = "$version";

/// What one `cargo xtask manifests` run was asked for.
#[derive(Debug, Clone, clap::Args)]
pub struct Options {
    /// The release's version, without the tag's `v`.
    #[arg(long)]
    pub version: String,
    /// The directory holding the release's archives, as `gh release download`
    /// leaves them.
    #[arg(long, value_name = "DIR")]
    pub assets: PathBuf,
    /// Where the three manifests are written, laid out as the two repositories.
    #[arg(long, value_name = "DIR")]
    pub out: PathBuf,
}

/// The five archives the manifests install from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asset {
    Windows,
    Linux(Arch),
    MacOsApp(Arch),
}

impl Asset {
    pub const ALL: [Self; 5] = [
        Self::Windows,
        Self::Linux(Arch::X86_64),
        Self::Linux(Arch::Aarch64),
        Self::MacOsApp(Arch::X86_64),
        Self::MacOsApp(Arch::Aarch64),
    ];

    /// The asset's name on the release page.
    pub fn file_name(self, version: &str) -> String {
        match self {
            Self::Windows => bundle::archive_name(version, Platform::Windows, Arch::X86_64),
            Self::Linux(arch) => bundle::archive_name(version, Platform::Linux, arch),
            Self::MacOsApp(arch) => bundle::app_archive_name(version, arch),
        }
    }
}

/// One release's five archives, hashed.
#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    hashes: Vec<(Asset, String)>,
}

impl Release {
    /// The SHA-256 of one archive, in lowercase hex.
    pub fn sha256(&self, asset: Asset) -> &str {
        self.hashes
            .iter()
            .find(|(a, _)| *a == asset)
            .map(|(_, hash)| hash.as_str())
            .expect("a release holds every asset, which `collect` checked")
    }

    fn url(&self, asset: Asset) -> String {
        download_url(&self.version, &asset.file_name(&self.version))
    }
}

/// Where a release asset is downloaded from once the release is published.
pub fn download_url(version: &str, name: &str) -> String {
    format!("{REPOSITORY}/releases/download/v{version}/{name}")
}

/// Refuse a version that is not one this project tags.
///
/// The version ends up inside Ruby string literals and URLs, so anything but
/// the characters a semantic version is made of is refused rather than escaped.
pub fn check_version(version: &str) -> Result<(), String> {
    if version.starts_with('v') {
        return Err(format!(
            "{version} is a tag; --version takes the version without its v, {}",
            version.trim_start_matches('v')
        ));
    }
    let well_formed = version.chars().next().is_some_and(|c| c.is_ascii_digit())
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    if well_formed {
        Ok(())
    } else {
        Err(format!(
            "{version:?} is not a version: it has to start with a digit and hold only \
             letters, digits, dots, hyphens and plus signs"
        ))
    }
}

/// The SHA-256 of a file, in lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0_u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Hash the five archives in `dir`, refusing a directory that lacks any.
pub fn collect(version: &str, dir: &Path) -> Result<Release, String> {
    check_version(version)?;
    let missing: Vec<String> = Asset::ALL
        .iter()
        .map(|asset| asset.file_name(version))
        .filter(|name| !dir.join(name).is_file())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "{} has no {}; `gh release download v{version}` fetches a release's assets",
            dir.display(),
            missing.join(", ")
        ));
    }
    let hashes = Asset::ALL
        .iter()
        .map(|&asset| Ok((asset, sha256_file(&dir.join(asset.file_name(version)))?)))
        .collect::<Result<_, String>>()?;
    Ok(Release {
        version: version.to_owned(),
        hashes,
    })
}

/// A name from the bundle's naming functions with the version and the
/// architecture replaced by Homebrew's tokens.
///
/// Derived from the aarch64 name and then checked against both, so a naming
/// change the tokens cannot express is refused instead of written.
fn arch_template(version: &str, name: impl Fn(&str, Arch) -> String) -> Result<String, String> {
    let template = name(VERSION_TOKEN, Arch::Aarch64).replacen(Arch::Aarch64.slug(), ARCH_TOKEN, 1);
    for arch in [Arch::X86_64, Arch::Aarch64] {
        let expanded = template
            .replace(VERSION_TOKEN, version)
            .replace(ARCH_TOKEN, arch.slug());
        let concrete = name(version, arch);
        if expanded != concrete {
            return Err(format!(
                "the cask's template {template} expands to {expanded} for {}, and the \
                 bundle names that archive {concrete}",
                arch.slug()
            ));
        }
    }
    Ok(template)
}

/// The cask for `Sunlit Earth.app`, macOS only.
pub fn cask(release: &Release) -> Result<String, String> {
    let version = &release.version;
    let archive = arch_template(version, bundle::app_archive_name)?;
    let directory = arch_template(version, bundle::app_bundle_name)?;
    let url = download_url(VERSION_TOKEN, &archive);
    let arm = release.sha256(Asset::MacOsApp(Arch::Aarch64));
    let intel = release.sha256(Asset::MacOsApp(Arch::X86_64));
    Ok(format!(
        r##"cask "{PACKAGE}" do
  arch arm: "{arm_slug}", intel: "{intel_slug}"

  version "{version}"
  sha256 arm:   "{arm}",
         intel: "{intel}"

  url "{url}"
  name "{APP_NAME}"
  desc "{DESCRIPTION}"
  homepage "{REPOSITORY}"

  livecheck do
    url "{REPOSITORY}"
    strategy :github_latest
  end

  depends_on macos: :big_sur

  app "{directory}/{APP_DIR}"
  binary "#{{appdir}}/{APP_DIR}/Contents/MacOS/{PACKAGE}"

  postflight_steps do
    run "/usr/bin/xattr", args: ["-dr", "com.apple.quarantine", "{{{{appdir}}}}/{APP_DIR}"]
  end

  uninstall quit: "{BUNDLE_ID}"

  zap trash: "{MACOS_DATA_DIR}"
end
"##,
        arm_slug = Arch::Aarch64.slug(),
        intel_slug = Arch::X86_64.slug(),
    ))
}

/// The formula for the Linux tarballs.
pub fn formula(release: &Release) -> String {
    let version = &release.version;
    let class = class_name(PACKAGE);
    let block = |arch: Arch| {
        let asset = Asset::Linux(arch);
        format!(
            "    url \"{}\"\n    sha256 \"{}\"\n",
            release.url(asset),
            release.sha256(asset)
        )
    };
    format!(
        r##"class {class} < Formula
  desc "{DESCRIPTION}"
  homepage "{REPOSITORY}"
  version "{version}"
  license "{LICENSE}"

  livecheck do
    url :stable
    strategy :github_latest
  end

  depends_on :linux

  on_arm do
{arm}  end
  on_intel do
{intel}  end

  def install
    libexec.install Dir["*"]
    bin.install_symlink libexec/"{PACKAGE}"
  end

  def caveats
    <<~EOS
      To add Sunlit Earth to your desktop's application menu, run:
        #{{opt_libexec}}/assets/linux/install-user.sh --exec #{{opt_bin}}/{PACKAGE}
      Before uninstalling, remove the entry again with:
        #{{opt_libexec}}/assets/linux/install-user.sh --uninstall
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{{bin}}/{PACKAGE} --version")
  end
end
"##,
        arm = block(Arch::Aarch64),
        intel = block(Arch::X86_64),
    )
}

/// Homebrew's class name for a formula name: `sunlit-earth` is `SunlitEarth`.
fn class_name(name: &str) -> String {
    name.split(['-', '_'])
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect()
}

#[derive(Serialize)]
struct ScoopManifest<'a> {
    version: &'a str,
    description: &'a str,
    homepage: &'a str,
    license: &'a str,
    notes: &'a str,
    architecture: ScoopArchitectures,
    bin: String,
    shortcuts: [[String; 2]; 1],
    checkver: &'a str,
    autoupdate: ScoopAutoupdate,
}

#[derive(Serialize)]
struct ScoopArchitectures {
    #[serde(rename = "64bit")]
    x64: ScoopArchitecture,
}

#[derive(Serialize)]
struct ScoopArchitecture {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hash: Option<String>,
    extract_dir: String,
}

#[derive(Serialize)]
struct ScoopAutoupdate {
    architecture: ScoopArchitectures,
}

/// The Scoop manifest for the Windows zip, indented as Scoop's own
/// `formatjson` writes it.
pub fn scoop(release: &Release) -> Result<String, String> {
    let version = &release.version;
    let exe = bundle::exe_name(Platform::Windows).to_owned();
    let manifest = ScoopManifest {
        version,
        description: DESCRIPTION,
        homepage: REPOSITORY,
        license: LICENSE,
        notes: SCOOP_NOTES,
        architecture: ScoopArchitectures {
            x64: ScoopArchitecture {
                url: release.url(Asset::Windows),
                hash: Some(release.sha256(Asset::Windows).to_owned()),
                extract_dir: bundle::bundle_name(version, Platform::Windows, Arch::X86_64),
            },
        },
        bin: exe.clone(),
        shortcuts: [[exe, APP_NAME.to_owned()]],
        checkver: "github",
        autoupdate: ScoopAutoupdate {
            architecture: ScoopArchitectures {
                x64: ScoopArchitecture {
                    url: download_url(
                        SCOOP_VERSION_TOKEN,
                        &Asset::Windows.file_name(SCOOP_VERSION_TOKEN),
                    ),
                    hash: None,
                    extract_dir: bundle::bundle_name(
                        SCOOP_VERSION_TOKEN,
                        Platform::Windows,
                        Arch::X86_64,
                    ),
                },
            },
        },
    };
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    manifest
        .serialize(&mut serializer)
        .map_err(|e| format!("cannot write the Scoop manifest: {e}"))?;
    let mut text = String::from_utf8(out).map_err(|e| format!("the manifest is not UTF-8: {e}"))?;
    text.push('\n');
    Ok(text)
}

/// Write the three manifests under `out`.
pub fn write_all(release: &Release, out: &Path) -> Result<Vec<PathBuf>, String> {
    let files = [
        (SCOOP_MANIFEST, scoop(release)?),
        (CASK, cask(release)?),
        (FORMULA, formula(release)),
    ];
    let mut written = Vec::new();
    for (relative, text) in files {
        let path = out.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

pub fn run(options: &Options) -> Result<u8, String> {
    let release = collect(&options.version, &options.assets)?;
    for asset in Asset::ALL {
        println!(
            "{}  {}",
            release.sha256(asset),
            asset.file_name(&release.version)
        );
    }
    for path in write_all(&release, &options.out)? {
        println!("wrote {}", path.display());
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSION: &str = "0.2.0";
    const PRERELEASE: &str = "0.2.0-beta.1";

    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        /// The five archives with contents that differ from each other, so a
        /// hash written against the wrong asset cannot pass.
        fn new(tag: &str, version: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("xtask_manifests_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("the fixture directory");
            for asset in Asset::ALL {
                let name = asset.file_name(version);
                std::fs::write(dir.join(&name), name.as_bytes()).expect("a fixture archive");
            }
            Self { dir }
        }

        fn release(&self, version: &str) -> Release {
            collect(version, &self.dir).expect("every archive is there")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Every download URL in a manifest, which is every `https://` string
    /// literal but the homepage and the livecheck page.
    fn urls(text: &str) -> Vec<String> {
        text.split('"')
            .filter(|part| part.starts_with("https://") && *part != REPOSITORY)
            .map(str::to_owned)
            .collect()
    }

    fn release_url(version: &str, name: &str) -> String {
        format!("https://github.com/sunlit-earth/sunlit-earth/releases/download/v{version}/{name}")
    }

    fn string_after<'a>(text: &'a str, stanza: &str) -> &'a str {
        let start = text
            .find(&format!("{stanza} \""))
            .unwrap_or_else(|| panic!("no {stanza} in\n{text}"))
            + stanza.len()
            + 2;
        let end = text[start..].find('"').expect("a closing quote") + start;
        &text[start..end]
    }

    fn expand(template: &str, version: &str, arch: Arch) -> String {
        template
            .replace("#{version}", version)
            .replace("#{arch}", arch.slug())
    }

    #[test]
    fn sha256_matches_the_published_test_vector() {
        let fixture = Fixture::new("vector", VERSION);
        let path = fixture.dir.join("abc");
        std::fs::write(&path, b"abc").expect("the vector");
        assert_eq!(
            sha256_file(&path).expect("a hash"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn each_hash_is_the_sha256_of_the_file_it_names() {
        let fixture = Fixture::new("hashes", VERSION);
        let release = fixture.release(VERSION);
        for asset in Asset::ALL {
            let name = asset.file_name(VERSION);
            let expected = hex(&Sha256::digest(name.as_bytes()));
            assert_eq!(release.sha256(asset), expected, "{name}");
        }

        let scoop: serde_json::Value =
            serde_json::from_str(&scoop(&release).expect("the manifest")).expect("JSON");
        assert_eq!(
            scoop["architecture"]["64bit"]["hash"],
            release.sha256(Asset::Windows)
        );
        let cask = cask(&release).expect("the cask");
        assert!(cask.contains(&format!(
            "arm:   \"{}\"",
            release.sha256(Asset::MacOsApp(Arch::Aarch64))
        )));
        assert!(cask.contains(&format!(
            "intel: \"{}\"",
            release.sha256(Asset::MacOsApp(Arch::X86_64))
        )));
        let formula = formula(&release);
        for arch in [Arch::X86_64, Arch::Aarch64] {
            let asset = Asset::Linux(arch);
            let url = release_url(VERSION, &asset.file_name(VERSION));
            let pair = format!("url \"{url}\"\n    sha256 \"{}\"", release.sha256(asset));
            assert!(formula.contains(&pair), "{formula}");
        }
    }

    #[test]
    fn every_url_is_a_release_download_of_an_asset_the_bundle_names() {
        for version in [VERSION, PRERELEASE] {
            let fixture = Fixture::new("urls", version);
            let release = fixture.release(version);

            let scoop = scoop(&release).expect("the manifest");
            let windows = Asset::Windows.file_name(version);
            let autoupdate = Asset::Windows.file_name("$version");
            assert_eq!(
                urls(&scoop),
                [
                    release_url(version, &windows),
                    release_url("$version", &autoupdate)
                ]
            );

            let cask = cask(&release).expect("the cask");
            let cask_urls = urls(&cask);
            assert_eq!(cask_urls.len(), 1, "{cask}");
            let version_in_cask = string_after(&cask, "version");
            assert_eq!(version_in_cask, version);
            for arch in [Arch::X86_64, Arch::Aarch64] {
                assert_eq!(
                    expand(&cask_urls[0], version, arch),
                    release_url(version, &bundle::app_archive_name(version, arch))
                );
            }

            let formula = formula(&release);
            let linux: Vec<String> = [Arch::Aarch64, Arch::X86_64]
                .into_iter()
                .map(|arch| release_url(version, &Asset::Linux(arch).file_name(version)))
                .collect();
            assert_eq!(urls(&formula), linux);
            assert_eq!(string_after(&formula, "version"), version);
        }
    }

    #[test]
    fn the_casks_app_is_the_bundle_directory_joined_with_the_app() {
        let fixture = Fixture::new("app", VERSION);
        let cask = cask(&fixture.release(VERSION)).expect("the cask");
        let app = string_after(&cask, "app");
        for arch in [Arch::X86_64, Arch::Aarch64] {
            assert_eq!(
                expand(app, VERSION, arch),
                format!(
                    "{}/Sunlit Earth.app",
                    bundle::app_bundle_name(VERSION, arch)
                )
            );
        }
        assert!(
            cask.contains("\"{{appdir}}/Sunlit Earth.app\"]"),
            "the quarantine removal names the installed app: {cask}"
        );
    }

    #[test]
    fn the_scoop_manifest_keeps_the_field_order_and_extracts_the_bundle_directory() {
        let fixture = Fixture::new("scoop", PRERELEASE);
        let text = scoop(&fixture.release(PRERELEASE)).expect("the manifest");
        assert!(text.ends_with("}\n"), "{text}");
        assert!(text.contains("\n    \"version\""), "four spaces: {text}");

        let value: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(value["version"], PRERELEASE);
        assert_eq!(
            value["architecture"]["64bit"]["extract_dir"],
            bundle::bundle_name(PRERELEASE, Platform::Windows, Arch::X86_64)
        );
        assert_eq!(
            value["autoupdate"]["architecture"]["64bit"]["extract_dir"],
            bundle::bundle_name("$version", Platform::Windows, Arch::X86_64)
        );

        let order = [
            "\"version\"",
            "\"description\"",
            "\"homepage\"",
            "\"license\"",
            "\"notes\"",
            "\"architecture\"",
            "\"bin\"",
            "\"shortcuts\"",
            "\"checkver\"",
            "\"autoupdate\"",
        ];
        let positions: Vec<usize> = order
            .iter()
            .map(|key| {
                text.find(&format!("\n    {key}"))
                    .unwrap_or_else(|| panic!("no top-level {key}"))
            })
            .collect();
        assert!(positions.is_sorted(), "{text}");
        let top_level = text.matches("\n    \"").count();
        assert_eq!(top_level, order.len(), "no other top-level key: {text}");
    }

    #[test]
    fn a_missing_archive_is_refused_by_name() {
        let fixture = Fixture::new("missing", VERSION);
        let gone = Asset::MacOsApp(Arch::X86_64).file_name(VERSION);
        std::fs::remove_file(fixture.dir.join(&gone)).expect("remove one");
        let refusal = collect(VERSION, &fixture.dir).expect_err("one is missing");
        assert!(refusal.contains(&gone), "{refusal}");
        for other in Asset::ALL
            .iter()
            .map(|asset| asset.file_name(VERSION))
            .filter(|name| *name != gone)
        {
            assert!(!refusal.contains(&other), "{refusal}");
        }
    }

    #[test]
    fn a_tag_or_a_malformed_version_is_refused() {
        assert!(check_version("0.2.0").is_ok());
        assert!(check_version("0.2.0-beta.1").is_ok());
        let tag = check_version("v0.2.0").expect_err("a tag");
        assert!(tag.contains("0.2.0"), "{tag}");
        for bad in ["", "beta", "0.2.0\"", "0.2.0 ", "0.2.0#{x}", "0.2/0"] {
            assert!(check_version(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_formula_class_is_homebrews_name_for_the_package() {
        assert_eq!(class_name("sunlit-earth"), "SunlitEarth");
        let fixture = Fixture::new("class", VERSION);
        assert!(formula(&fixture.release(VERSION)).starts_with("class SunlitEarth < Formula\n"));
    }

    #[test]
    fn the_license_is_the_workspaces() {
        let manifest = std::fs::read_to_string(crate::store::repo_root().join("Cargo.toml"))
            .expect("the workspace manifest");
        let file: toml::Value = toml::from_str(&manifest).expect("TOML");
        assert_eq!(
            file["workspace"]["package"]["license"].as_str(),
            Some(LICENSE)
        );
    }

    #[test]
    fn the_bundle_identifier_is_the_info_plists() {
        let plist = std::fs::read_to_string(crate::store::repo_root().join(bundle::INFO_PLIST))
            .expect("the plist template");
        assert!(
            plist.contains(&format!("<string>{BUNDLE_ID}</string>")),
            "{plist}"
        );
    }
}
