// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Russet contributors

//! `cargo xtask pins`: generates `pins/stable.toml` from upstream release metadata and signs it with minisign
//! (ARCHITECTURE §14.7) [D 87].
//!
//! Wave 1 (W3) covers the two components W3 installs: `python-shared` (python-build-standalone 3.12.15, release
//! 20261009) and `darwin-tools` 1.1.0. For each, the asset URLs and sizes come from the GitHub release metadata
//! (`gh api repos/<owner>/<repo>/releases/tags/<tag>`), and the `sha256` from hashing the downloaded asset, which
//! is streamed and never stored; a digest GitHub publishes must agree. Nothing is hand-typed except the asset
//! naming pattern and the curated `[[xcode]]` compatibility rows. W26 extends this to every component and moves
//! generation into CI with the release key.
//!
//! Signing uses a minisign-compatible CLI (`minisign` or `rsign` from rsign2; `--signer` or `RUSSET_MINISIGN`)
//! with the development key `~/.config/russet-dev/minisign.key`, whose public half is `pins/dev-minisign.pub`.
//! `--init-dev-key` creates that key pair once (passwordless, secret 0600, never committed).
//!
//! ```sh
//! cargo xtask pins --init-dev-key            # once per machine
//! cargo xtask pins                           # regenerate and sign pins/stable.toml
//! cargo xtask pins --platform linux-x86_64   # fewer downloads while iterating (other platforms are dropped)
//! ```
//! Owner: W3 (Wave 1), then W26.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::workstreams;

/// Flags of `cargo xtask pins`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Output file (default: pins/stable.toml in the repository)
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Only these platform keys (linux-x86_64, linux-aarch64); default: every platform of every component
    #[arg(long = "platform", value_name = "KEY")]
    pub platforms: Vec<String>,
    /// The manifest id (default: today's date, YYYY.MM.DD)
    #[arg(long)]
    pub id: Option<String>,
    /// The minisign secret key (default: ~/.config/russet-dev/minisign.key)
    #[arg(long)]
    pub key: Option<PathBuf>,
    /// A minisign-compatible CLI (`minisign` or `rsign`; default: $RUSSET_MINISIGN, then PATH)
    #[arg(long)]
    pub signer: Option<PathBuf>,
    /// Write the manifest without signing it
    #[arg(long)]
    pub no_sign: bool,
    /// Create the development key pair (if missing) and exit
    #[arg(long)]
    pub init_dev_key: bool,
}

/// One upstream release the manifest pins.
struct Source {
    component: &'static str,
    version: &'static str,
    source: &'static str,
    release: Option<&'static str>,
    repo: &'static str,
    tag: &'static str,
    /// Platform key → asset name in the release.
    assets: &'static [(&'static str, &'static str)],
}

/// The Wave-1 components (ARCHITECTURE §14.7, §16.7).
const SOURCES: &[Source] = &[
    Source {
        component: "python-shared",
        version: "3.12.15",
        source: "python-build-standalone",
        release: Some("20261009"),
        repo: "astral-sh/python-build-standalone",
        tag: "20261009",
        assets: &[
            ("linux-x86_64", "cpython-3.12.15+20261009-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz"),
            ("linux-aarch64", "cpython-3.12.15+20261009-aarch64-unknown-linux-gnu-install_only_stripped.tar.gz"),
        ],
    },
    Source {
        component: "darwin-tools",
        version: "1.1.0",
        source: "github:xtool-org/darwin-tools-linux-llvm@v1.1.0",
        release: None,
        repo: "xtool-org/darwin-tools-linux-llvm",
        tag: "v1.1.0",
        assets: &[("linux-x86_64", "toolset-x86_64.tar.gz"), ("linux-aarch64", "toolset-aarch64.tar.gz")],
    },
];

/// The curated Xcode compatibility rows (core §10.2–§10.3; verified on the dev box, core §23): an SDK's Xcode
/// version selects the Swift minor and the linker floor.
fn xcode_rows() -> Vec<XcodeCompat> {
    vec![XcodeCompat {
        version: "27.0".into(),
        build: "27A266a".into(),
        ios_sdk: "27.0".into(),
        swift: "6.4".into(),
        min_lld: "23.1.3".into(),
        min_darwin_tools: Some("1.1.0".into()),
    }]
}

// The manifest shape; russet-toolchain's `pins` module parses it (and its tests check the committed file).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Manifest {
    schema: u32,
    id: String,
    min_russet: String,
    generated_at: String,
    xcode: Vec<XcodeCompat>,
    component: Vec<ComponentPin>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct XcodeCompat {
    version: String,
    build: String,
    ios_sdk: String,
    swift: String,
    min_lld: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_darwin_tools: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct ComponentPin {
    name: String,
    version: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    release: Option<String>,
    artifacts: BTreeMap<String, Artifact>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Artifact {
    url: String,
    size: u64,
    sha256: String,
    format: String,
}

/// The parts of a GitHub release the generator reads.
#[derive(Debug, Deserialize)]
struct Release {
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    size: u64,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

pub fn run(args: Args) -> Result<()> {
    let root = workstreams::repo_root();
    let key = args.key.clone().unwrap_or_else(default_key_path);
    let pubkey = root.join("pins/dev-minisign.pub");
    if args.init_dev_key {
        return init_dev_key(&args, &key, &pubkey);
    }
    let out = args.out.clone().unwrap_or_else(|| root.join("pins/stable.toml"));
    for p in &args.platforms {
        ensure!(SOURCES.iter().any(|s| s.assets.iter().any(|(k, _)| k == p)), "unknown platform key {p}");
    }

    let mut components = Vec::new();
    for src in SOURCES {
        let release = fetch_release(src.repo, src.tag)?;
        let mut artifacts = BTreeMap::new();
        for (platform, asset_name) in src.assets {
            if !args.platforms.is_empty() && !args.platforms.iter().any(|p| p == platform) {
                continue;
            }
            let asset = release
                .assets
                .iter()
                .find(|a| a.name == *asset_name)
                .with_context(|| format!("{}@{} has no asset {asset_name}", src.repo, src.tag))?;
            eprintln!("pins: hashing {} ({} bytes)", asset.name, asset.size);
            let (size, sha256) = download_and_hash(&asset.browser_download_url)?;
            ensure!(size == asset.size, "{}: downloaded {size} bytes, the release says {}", asset.name, asset.size);
            if let Some(d) = &asset.digest
                && let Some(published) = d.strip_prefix("sha256:")
            {
                ensure!(published == sha256, "{}: sha256 {sha256} differs from GitHub's {published}", asset.name);
            }
            let format = if asset.name.ends_with(".tar.gz") { "tar.gz" } else { "unknown" };
            artifacts.insert(
                (*platform).to_owned(),
                Artifact { url: asset.browser_download_url.clone(), size, sha256, format: format.into() },
            );
        }
        components.push(ComponentPin {
            name: src.component.into(),
            version: src.version.into(),
            source: src.source.into(),
            release: src.release.map(Into::into),
            artifacts,
        });
    }

    let today = utc_date()?;
    let manifest = Manifest {
        schema: 1,
        id: args.id.clone().unwrap_or_else(|| today.replace('-', ".")),
        min_russet: "0.1.0".into(),
        generated_at: format!("{today}T00:00:00Z"),
        xcode: xcode_rows(),
        component: components,
    };
    let body = toml::to_string_pretty(&manifest).context("serializing the manifest")?;
    let text = format!(
        "# GENERATED by `cargo xtask pins` (xtask/src/pins.rs) from upstream release metadata; do not edit.\n\
         # Signed with minisign: pins/stable.toml.minisig. ARCHITECTURE §14.7 [D 87].\n\n{body}"
    );
    std::fs::write(&out, &text).with_context(|| format!("writing {}", out.display()))?;
    eprintln!("pins: wrote {}", out.display());
    if args.no_sign {
        return Ok(());
    }
    let sig = out.with_extension("toml.minisig");
    sign(&args, &key, &out, &sig, &manifest.id)?;
    eprintln!("pins: signed {}", sig.display());
    Ok(())
}

fn default_key_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".config/russet-dev/minisign.key")
}

/// Today's UTC date as `YYYY-MM-DD` (`date -u`, so xtask needs no date crate).
fn utc_date() -> Result<String> {
    let out = Command::new("date").args(["-u", "+%Y-%m-%d"]).output().context("running `date`")?;
    ensure!(out.status.success(), "`date -u` failed");
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}

/// The release metadata: `gh api` when gh is installed, else the public API through curl.
fn fetch_release(repo: &str, tag: &str) -> Result<Release> {
    let endpoint = format!("repos/{repo}/releases/tags/{tag}");
    let out = match Command::new("gh").args(["api", &endpoint]).output() {
        Ok(o) => o,
        Err(_) => Command::new("curl")
            .args(["-fsSL", "--proto", "=https", &format!("https://api.github.com/{endpoint}")])
            .output()
            .context("neither gh nor curl could be run")?,
    };
    ensure!(out.status.success(), "reading {endpoint} failed: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).with_context(|| format!("parsing {endpoint}"))
}

/// Streams `url` through curl into SHA-256; nothing is written to disk.
fn download_and_hash(url: &str) -> Result<(u64, String)> {
    let mut child = Command::new("curl")
        .args(["-fsSL", "--proto", "=https", "--tlsv1.2", url])
        .stdout(Stdio::piped())
        .spawn()
        .context("running curl")?;
    let mut stdout = child.stdout.take().context("curl stdout")?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = stdout.read(&mut buf).context("reading the download")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    let status = child.wait().context("waiting for curl")?;
    ensure!(status.success(), "downloading {url} failed ({status})");
    let digest = hasher.finalize();
    Ok((size, digest.iter().map(|b| format!("{b:02x}")).collect()))
}

/// The minisign-compatible signer and its flavor.
fn signer(args: &Args) -> Result<(PathBuf, bool)> {
    let explicit = args.signer.clone().or_else(|| std::env::var_os("RUSSET_MINISIGN").map(PathBuf::from));
    let found = match explicit {
        Some(p) => p.is_file().then_some(p),
        None => ["minisign", "rsign"].iter().find_map(|name| find_in_path(name)),
    };
    if let Some(c) = found {
        let is_rsign = c.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rsign"));
        return Ok((c, is_rsign));
    }
    bail!("no minisign-compatible signer found: install minisign or rsign2 (`cargo install rsign2`), or pass --signer")
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

fn sign(args: &Args, key: &Path, file: &Path, sig: &Path, id: &str) -> Result<()> {
    ensure!(key.is_file(), "no signing key at {} (run `cargo xtask pins --init-dev-key`)", key.display());
    let (bin, is_rsign) = signer(args)?;
    let trusted = format!("russet pins {id} (development key)");
    let status = if is_rsign {
        Command::new(&bin)
            .arg("sign")
            .arg("-W")
            .arg("-s")
            .arg(key)
            .arg("-x")
            .arg(sig)
            .arg("-t")
            .arg(&trusted)
            .arg(file)
            .status()
    } else {
        Command::new(&bin)
            .arg("-S")
            .arg("-s")
            .arg(key)
            .arg("-m")
            .arg(file)
            .arg("-x")
            .arg(sig)
            .arg("-t")
            .arg(&trusted)
            .status()
    }
    .with_context(|| format!("running {}", bin.display()))?;
    ensure!(status.success(), "{} failed to sign {}", bin.display(), file.display());
    Ok(())
}

fn init_dev_key(args: &Args, key: &Path, pubkey: &Path) -> Result<()> {
    if key.is_file() {
        ensure!(pubkey.is_file(), "{} exists but {} does not", key.display(), pubkey.display());
        eprintln!("pins: the development key already exists ({})", key.display());
        return Ok(());
    }
    let dir = key.parent().context("key path has no parent")?;
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let (bin, is_rsign) = signer(args)?;
    let comment = "russet development pins key (never for releases)";
    let status = if is_rsign {
        Command::new(&bin).args(["generate", "-W", "-c", comment, "-p"]).arg(pubkey).arg("-s").arg(key).status()
    } else {
        Command::new(&bin).args(["-G", "-W", "-p"]).arg(pubkey).arg("-s").arg(key).status()
    }
    .with_context(|| format!("running {}", bin.display()))?;
    ensure!(status.success(), "{} failed to generate the key pair", bin.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(key, std::fs::Permissions::from_mode(0o600))?;
    }
    eprintln!("pins: created {} (secret, 0600) and {} (public)", key.display(), pubkey.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_names_assets_for_known_platforms() {
        for s in SOURCES {
            assert!(!s.assets.is_empty(), "{}", s.component);
            for (platform, asset) in s.assets {
                assert!(matches!(*platform, "linux-x86_64" | "linux-aarch64"), "{platform}");
                assert!(asset.ends_with(".tar.gz"), "{asset}");
                assert!(asset.contains(if platform.ends_with("x86_64") { "x86_64" } else { "aarch64" }));
            }
        }
    }

    #[test]
    fn the_committed_manifest_matches_the_generator_shape() {
        let root = workstreams::repo_root();
        let text = std::fs::read_to_string(root.join("pins/stable.toml")).expect("pins/stable.toml");
        let m: Manifest = toml::from_str(&text).expect("the committed manifest parses");
        assert_eq!(m.schema, 1);
        for s in SOURCES {
            let c = m.component.iter().find(|c| c.name == s.component).expect(s.component);
            assert_eq!(c.version, s.version);
            for a in c.artifacts.values() {
                assert!(a.url.starts_with("https://github.com/"), "{}", a.url);
                assert_eq!(a.sha256.len(), 64);
            }
        }
    }
}
