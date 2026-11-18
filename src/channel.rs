//! The local release channel of the install scenario: the release medium carries no
//! package cache, so recsetup's software step fetches the release's category parts with
//! `recupdate fetch-parts` (docs/release-engineering.md, section 11). The scenario does the
//! same, against the host instead of GitHub:
//!
//! - the release run directory holds the unsigned `release.json` and `parts-*.tar.zst`;
//! - this module fills each part's url, sha256 and size for a local http base (the fill
//!   `cargo xtask release publish` does for GitHub; xtask depends on this crate, so the few
//!   lines are kept here, for parts only: `fetch-parts` reads nothing else), signs the
//!   manifest with a throwaway Ed25519 key made for this run, and serves manifest, signature
//!   and parts from the host;
//! - the live system trusts only that key (`RECUPDATE_KEYS_DIR` names a folder with its
//!   `.pub`, not the production key folder) and reaches the host through its default
//!   gateway, which the VM network (passt) maps to the host's loopback.
//!
//! The parts are served from the run directory itself; nothing is copied.

use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer, SigningKey};
use recqemu::shell_quote;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// The release's own record on the medium: os and serial, which `fetch-parts` requires the
/// fetched manifest to match exactly. Mirrors recupdate's installed-release path.
pub const MEDIUM_RELEASE_RECORD: &str = "/usr/lib/levitate/release.json";
/// The folder of the production public keys on the medium (recupdate's default key folder).
pub const MEDIUM_RELEASE_KEYS: &str = "/usr/share/levitate/release-keys";
/// Where the scenario puts the throwaway public key in the live system.
const GUEST_KEYS_DIR: &str = "/run/scenario-release-keys";
/// Where the scenario puts the signed manifest, without the parts, for the negative check.
const GUEST_MANIFEST_DIR: &str = "/run/scenario-release-manifest";

/// What the server hands out under one name.
enum Served {
    Bytes(Vec<u8>),
    File(PathBuf),
}

/// A signed local channel being served: the base address the live system uses, and what the
/// live system needs to trust it.
pub struct Channel {
    base: String,
    public_key: String,
    manifest: Vec<u8>,
    signature: String,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
}

impl Channel {
    /// Fill, sign and serve the release in `release_dir` for a live system that reaches the
    /// host at `gateway` (its default gateway, an IPv4 address).
    pub fn start(release_dir: &Path, gateway: &str) -> Result<Self> {
        let gateway: Ipv4Addr = gateway
            .trim()
            .parse()
            .with_context(|| format!("the live system's default gateway {gateway:?} is not an IPv4 address; it needs a default route to reach the host"))?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).context("binding the host's local release channel")?;
        let port = listener.local_addr()?.port();
        let base = format!("http://{gateway}:{port}/");

        let mut seed = [0u8; 32];
        fs::File::open("/dev/urandom")?.read_exact(&mut seed)?;
        let key = SigningKey::from_bytes(&seed);
        let (manifest, files) = fill_manifest(release_dir, &base)?;
        let signature = format!("{}\n", STANDARD.encode(key.sign(&manifest).to_bytes()));

        let mut served: HashMap<String, Served> = files
            .into_iter()
            .map(|(name, path)| (name, Served::File(path)))
            .collect();
        served.insert("release.json".into(), Served::Bytes(manifest.clone()));
        served.insert("release.json.sig".into(), Served::Bytes(signature.clone().into_bytes()));

        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let served = Arc::new(served);
        let server = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let served = served.clone();
                            std::thread::spawn(move || {
                                let _ = answer(stream, &served);
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        Ok(Self {
            base,
            public_key: STANDARD.encode(key.verifying_key().to_bytes()),
            manifest,
            signature,
            stop,
            server: Some(server),
        })
    }

    /// The shell command that puts the throwaway public key (and, for the negative check,
    /// the signed manifest without its parts) into the live system.
    pub fn guest_setup(&self) -> String {
        format!(
            "mkdir -p {keys} {manifest} && printf '%s\\n' {public} > {keys}/scenario.pub \
             && printf '%s' {release} | base64 -d > {manifest}/release.json \
             && printf '%s' {signature} > {manifest}/release.json.sig",
            keys = GUEST_KEYS_DIR,
            manifest = GUEST_MANIFEST_DIR,
            public = shell_quote(&self.public_key),
            release = shell_quote(&STANDARD.encode(&self.manifest)),
            signature = shell_quote(&self.signature),
        )
    }

    /// The environment that points `recupdate fetch-parts` (through recsetup, which hands
    /// its own environment on) at this channel and at the throwaway key alone.
    pub fn guest_env(&self) -> String {
        format!(
            "RECUPDATE_RELEASE_BASE_URL={} RECUPDATE_KEYS_DIR={GUEST_KEYS_DIR}",
            shell_quote(&self.base)
        )
    }

    /// A folder in the live system holding the signed manifest and no part: the way a
    /// missing part is shown (`recsetup apply --parts-from`).
    pub fn guest_manifest_dir(&self) -> &'static str {
        GUEST_MANIFEST_DIR
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

/// The part files of a release, as (name served under, path on disk).
type PartFiles = Vec<(String, PathBuf)>;

/// The unsigned `release.json` of `dir` with every part's url, sha256 and size filled for
/// `base`, as the bytes to sign, and the part files by the name they are served under.
fn fill_manifest(dir: &Path, base: &str) -> Result<(Vec<u8>, PartFiles)> {
    let path = dir.join("release.json");
    let mut manifest: Value = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
    )
    .with_context(|| format!("{} is not JSON", path.display()))?;
    ensure!(manifest["schema"] == 1, "{} must have \"schema\": 1", path.display());
    let parts = manifest["parts"]
        .as_array_mut()
        .filter(|parts| !parts.is_empty())
        .with_context(|| format!("{} lists no parts; build the release again", path.display()))?;
    let mut files = Vec::new();
    for part in parts.iter_mut() {
        let name = part["name"]
            .as_str()
            .filter(|n| n.starts_with("parts-") && !n.contains(['/', '.']))
            .context("release.json: a part has no name of the form parts-<category>")?
            .to_owned();
        let served = format!("{name}.tar.zst");
        let file = dir.join(&served);
        ensure!(
            file.is_file(),
            "{} is missing; the release build writes one parts-<category>.tar.zst per category next to release.json",
            file.display()
        );
        let mut input = fs::File::open(&file).with_context(|| format!("opening {}", file.display()))?;
        let mut hash = Sha256::new();
        let size = std::io::copy(&mut input, &mut hash)?;
        part["url"] = json!(format!("{base}{served}"));
        part["sha256"] = json!(format!("{:x}", hash.finalize()));
        part["size"] = json!(size);
        files.push((served, file));
    }
    Ok((serde_json::to_vec_pretty(&manifest)?, files))
}

/// Answer one request: GET (and HEAD) of a served name, with `Range: bytes=N-` for resuming.
fn answer(mut stream: TcpStream, served: &HashMap<String, Served>) -> Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    stream.set_write_timeout(Some(Duration::from_secs(120)))?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        ensure!(head.len() < 16 * 1024, "request head too large");
        if stream.read(&mut byte)? == 0 {
            bail!("connection closed inside the request head");
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let mut lines = head.lines();
    let mut request = lines.next().unwrap_or_default().split_whitespace();
    let (method, target) = (request.next().unwrap_or_default(), request.next().unwrap_or_default());
    let start = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("range"))
        .and_then(|(_, value)| value.trim().strip_prefix("bytes=")?.strip_suffix('-')?.parse::<u64>().ok());
    let respond = |stream: &mut TcpStream, status: &str, extra: &str, length: u64| {
        write!(stream, "HTTP/1.1 {status}\r\n{extra}Content-Length: {length}\r\nConnection: close\r\n\r\n")
    };
    let name = target.trim_start_matches('/');
    let (Some(item), true) = (served.get(name), method == "GET" || method == "HEAD") else {
        respond(&mut stream, "404 Not Found", "", 0)?;
        return Ok(());
    };
    let total = match item {
        Served::Bytes(bytes) => bytes.len() as u64,
        Served::File(path) => fs::metadata(path)?.len(),
    };
    let from = start.unwrap_or(0);
    if from > total {
        respond(&mut stream, "416 Range Not Satisfiable", &format!("Content-Range: bytes */{total}\r\n"), 0)?;
        return Ok(());
    }
    let mut body: Box<dyn Read> = match item {
        Served::Bytes(bytes) => Box::new(std::io::Cursor::new(bytes[from as usize..].to_vec())),
        Served::File(path) => {
            let mut file = fs::File::open(path)?;
            file.seek(SeekFrom::Start(from))?;
            Box::new(file)
        }
    };
    if start.is_some() {
        respond(
            &mut stream,
            "206 Partial Content",
            &format!("Content-Range: bytes {from}-{}/{total}\r\n", total.saturating_sub(1)),
            total - from,
        )?;
    } else {
        respond(&mut stream, "200 OK", "", total)?;
    }
    if method == "GET" {
        std::io::copy(&mut body, &mut stream)?;
    }
    stream.flush()?;
    Ok(())
}
