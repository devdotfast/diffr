//! Only the embedded catalog grants trust; installation never discovers versions.
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use strum::IntoEnumIterator;

use crate::parse::guess_language::Language;
use crate::summary::FallbackCause;

pub(crate) const TARGET: &str = env!("DIFFR_TARGET");
const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
const MAX_FILE: u64 = 128 * 1024 * 1024;
const MAX_UNPACKED: u64 = 256 * 1024 * 1024;

#[derive(Deserialize)]
struct Catalog {
    schema: u32,
    packages: Vec<Package>,
}
#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct Library {
    pub id: String,
    pub file: String,
    pub symbol: String,
    pub abi: usize,
}
#[derive(Deserialize, Serialize, Clone)]
struct Fingerprint {
    size: u64,
    sha256: String,
}
#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct Package {
    schema: u32,
    pack: String,
    pub version: String,
    target: String,
    name: String,
    url: String,
    size: u64,
    sha256: String,
    pub libraries: Vec<Library>,
    files: BTreeMap<String, Fingerprint>,
}
static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    let catalog: Catalog = serde_json::from_str(include_str!("../languages/catalog.json"))
        .expect("invalid embedded language catalog");
    assert_eq!(catalog.schema, 1);
    catalog
});

pub(crate) use crate::parse::optional::{builtin, id as optional_id};
pub(crate) fn package() -> Option<&'static Package> {
    CATALOG.packages.iter().find(|p| p.target == TARGET)
}
fn root() -> Result<PathBuf> {
    let path = std::env::var_os("DIFFR_PARSER_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|p| p.join("diffr/parsers")))
        .context("cannot determine parser store")?;
    Ok(std::path::absolute(path)?)
}
pub(crate) fn directory(package: &Package) -> Result<PathBuf> {
    Ok(root()?
        .join(&package.target)
        .join("extra")
        .join(&package.version))
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read_verified(path: &Path, fingerprint: &Fingerprint) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "not a regular file: {}",
        path.display()
    );
    ensure!(
        fingerprint.size <= MAX_FILE && metadata.len() == fingerprint.size,
        "wrong size: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    File::open(path)?
        .take(fingerprint.size + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == fingerprint.size && hash(&bytes) == fingerprint.sha256,
        "integrity failure: {}",
        path.display()
    );
    Ok(bytes)
}
pub(crate) fn verify_library(
    package: &Package,
    library: &Library,
    directory: &Path,
) -> Result<PathBuf> {
    let expected = fs::canonicalize(root()?)?
        .join(&package.target)
        .join("extra")
        .join(&package.version);
    ensure!(
        fs::canonicalize(directory)? == expected,
        "linked pack directory outside parser store"
    );
    let path = directory.join(&library.file);
    read_verified(
        &path,
        package
            .files
            .get(&library.file)
            .context("uncataloged library")?,
    )?;
    let canonical = fs::canonicalize(&path)?;
    ensure!(
        canonical.parent() == Some(fs::canonicalize(directory)?.as_path()),
        "library outside parser store"
    );
    Ok(canonical)
}

pub(crate) fn highlights(package: &Package, id: &str, directory: &Path) -> Result<String> {
    let name = format!("{id}.highlights.scm");
    let bytes = read_verified(
        &directory.join(&name),
        package
            .files
            .get(&name)
            .context("missing catalog highlights")?,
    )?;
    Ok(String::from_utf8(bytes)?)
}
fn verify_install(package: &Package, directory: &Path) -> Result<()> {
    ensure!(
        !fs::symlink_metadata(directory)?.file_type().is_symlink(),
        "linked pack directory"
    );
    for (file, fingerprint) in &package.files {
        read_verified(&directory.join(file), fingerprint)?;
    }
    Ok(())
}
pub(crate) fn lock(package: &Package) -> Result<File> {
    let directory = directory(package)?;
    let parent = directory.parent().unwrap();
    fs::create_dir_all(parent)?;
    let expected = fs::canonicalize(root()?)?
        .join(&package.target)
        .join("extra");
    ensure!(
        fs::canonicalize(parent)? == expected,
        "linked directory outside parser store"
    );
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(parent.join(format!(".{}.lock", package.version)))?;
    lock.lock_exclusive()?;
    Ok(lock)
}

#[derive(Debug, Clone)]
pub(crate) struct ParserError {
    pub cause: FallbackCause,
    pub message: String,
}
impl ParserError {
    pub(crate) fn new(cause: FallbackCause, detail: impl std::fmt::Display) -> Self {
        Self { cause, message: format!("{detail}; run `diffr languages install extra` to install or repair additional language support") }
    }
}
impl std::fmt::Display for ParserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ParserError {}

fn unpack(package: &Package, archive: &Path, destination: &Path) -> Result<()> {
    ensure!(package.size <= MAX_ARCHIVE, "archive exceeds limit");
    let bytes = read_verified(
        archive,
        &Fingerprint {
            size: package.size,
            sha256: package.sha256.clone(),
        },
    )?;
    ensure!(package.size <= MAX_ARCHIVE, "archive exceeds limit");
    let decoder = flate2::read::GzDecoder::new(bytes.as_slice());
    let mut tar = tar::Archive::new(decoder.take(MAX_UNPACKED + 1));
    let mut seen = BTreeSet::new();
    for entry in tar.entries()? {
        let mut entry = entry?;
        ensure!(
            entry.header().entry_type().is_file(),
            "archive contains non-file entry"
        );
        let path = entry.path_bytes();
        let path = std::str::from_utf8(&path)?;
        let name = path
            .strip_prefix("package/")
            .context("invalid archive prefix")?
            .to_owned();
        ensure!(
            !name.contains('/') && !name.contains('\\') && name != "." && name != "..",
            "invalid archive path"
        );
        let expected = package
            .files
            .get(&name)
            .context("unexpected archive file")?;
        ensure!(seen.insert(name.clone()), "duplicate archive file");
        ensure!(
            entry.size() == expected.size && expected.size <= MAX_FILE,
            "archive file exceeds limit"
        );
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == expected.size && hash(&bytes) == expected.sha256,
            "archive file integrity failure"
        );
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(destination.join(name))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    ensure!(seen.len() == package.files.len(), "incomplete archive");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(destination.join("manifest.json"))?)?;
    ensure!(
        manifest["schema"] == 1
            && manifest["pack"] == "extra"
            && manifest["target"] == package.target
            && manifest["version"] == package.version
            && manifest["libraries"] == serde_json::to_value(&package.libraries)?,
        "incompatible manifest"
    );
    Ok(())
}
fn install(package: &Package, archive: Option<&Path>) -> Result<()> {
    ensure!(
        package.schema == 1 && package.pack == "extra" && package.target == TARGET,
        "incompatible package"
    );
    ensure!(package.size <= MAX_ARCHIVE, "archive exceeds limit");
    let _lock = lock(package)?;
    let destination = directory(package)?;
    if verify_install(package, &destination).is_ok() {
        return Ok(());
    }
    let parent = destination.parent().unwrap();
    let staging = tempfile::tempdir_in(parent)?;
    let download = staging.path().join("archive.tgz");
    let archive = match archive {
        Some(path) => path,
        None => {
            ensure!(
                package
                    .url
                    .starts_with("https://registry.npmjs.org/@dev.fast/diffr-languages-extra-"),
                "invalid catalog URL"
            );
            eprintln!(
                "Downloading extra {} ({} bytes)",
                package.version, package.size
            );
            tokio::runtime::Runtime::new()?.block_on(download_archive(
                &package.url,
                package.size,
                &download,
            ))?;
            &download
        }
    };
    let prepared = staging.path().join("prepared");
    fs::create_dir(&prepared)?;
    unpack(package, archive, &prepared)?;
    File::open(&prepared)?.sync_all()?;
    publish_directory(&prepared, &destination)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

async fn download_archive(url: &str, expected_size: u64, destination: &Path) -> Result<()> {
    ensure!(expected_size <= MAX_ARCHIVE, "archive exceeds limit");
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(180))
        .build()?;
    let mut response = client.get(url).send().await?.error_for_status()?;
    let mut file = File::create(destination)?;
    let mut size = 0;
    while let Some(chunk) = response.chunk().await? {
        size += chunk.len() as u64;
        ensure!(size <= expected_size, "download exceeds pinned size");
        file.write_all(&chunk)?;
    }
    ensure!(size == expected_size, "incomplete download");
    file.sync_all()?;
    Ok(())
}

fn publish_directory(prepared: &Path, destination: &Path) -> Result<()> {
    match fs::symlink_metadata(destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::rename(prepared, destination)?;
            return Ok(());
        }
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    // Exchange complete directories so interruption cannot expose a partial repair.
    #[cfg(any(target_os = "macos", all(target_os = "linux", target_env = "gnu")))]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(prepared.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
        #[cfg(target_os = "macos")]
        let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_SWAP) };
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
    anyhow::bail!("atomic pack repair is unsupported on this target")
}

pub(crate) fn run() -> Result<i32> {
    let command = clap::Command::new("diffr languages")
        .subcommand_required(true)
        .subcommand(
            clap::Command::new("list").arg(
                clap::Arg::new("json")
                    .long("json")
                    .action(clap::ArgAction::SetTrue),
            ),
        )
        .subcommand(
            clap::Command::new("install")
                .arg(
                    clap::Arg::new("pack")
                        .required(true)
                        .value_parser(["extra"]),
                )
                .arg(
                    clap::Arg::new("json")
                        .long("json")
                        .action(clap::ArgAction::SetTrue),
                )
                .arg(
                    clap::Arg::new("archive")
                        .long("archive")
                        .value_parser(clap::value_parser!(PathBuf)),
                ),
        );
    let args = command.try_get_matches_from(
        std::iter::once(std::ffi::OsString::from("diffr languages"))
            .chain(std::env::args_os().skip(2)),
    );
    let args = match args {
        Ok(args) => args,
        Err(error) => {
            let code = error.exit_code();
            error.print()?;
            return Ok(code);
        }
    };
    let (action, args) = args.subcommand().unwrap();
    let json = args.get_flag("json");
    if action == "list" {
        let package = package();
        let state = package
            .map(|p| directory(p).and_then(|dir| verify_install(p, &dir)))
            .transpose();
        let languages: Vec<_> = Language::iter()
            .filter(|language| optional_id(*language).is_some())
            .map(|language| {
                let availability = if builtin(language) {
                    "built_in"
                } else if package.is_none() {
                    "unavailable"
                } else if state.as_ref().is_ok_and(|s| s.is_some()) {
                    "installed"
                } else if package
                    .and_then(|p| directory(p).ok())
                    .is_some_and(|p| p.exists())
                {
                    "corrupt"
                } else {
                    "not_installed"
                };
                serde_json::json!({"id": optional_id(language), "availability": availability})
            })
            .collect();
        let result = serde_json::json!({"target": TARGET, "pack": "extra", "revision": package.map(|p| &p.version), "download_size": package.map(|p| p.size), "languages": languages});
        if json {
            println!("{result}");
        } else {
            match package {
                Some(pack) => println!("extra {}: {} ({} bytes)", pack.version, TARGET, pack.size),
                None => println!("extra: no published revision for {TARGET}"),
            }
            for language in languages {
                println!(
                    "{}: {}",
                    language["id"].as_str().unwrap(),
                    language["availability"].as_str().unwrap()
                );
            }
        }
        return Ok(0);
    }
    let result = package()
        .context("no published extra pack is pinned for this executable target")
        .and_then(|p| install(p, args.get_one::<PathBuf>("archive").map(PathBuf::as_path)));
    match result {
        Ok(()) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"ok": true, "pack": "extra", "target": TARGET, "revision": package().unwrap().version})
                );
            } else {
                println!("Installed extra {}", package().unwrap().version);
            }
            Ok(0)
        }
        Err(error) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"ok": false, "error": {"code": "language_install_failed", "message": format!("{error:#}")}})
                );
            } else {
                eprintln!("{error:#}");
            }
            Ok(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};

    fn fixture(entries: &[(&str, &[u8], tar::EntryType)]) -> (tempfile::TempDir, PathBuf, Package) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pack.tgz");
        let encoder = GzEncoder::new(File::create(&path).unwrap(), Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let manifest = serde_json::json!({"schema": 1, "pack": "extra", "version": "0.0.0", "target": TARGET, "libraries": []}).to_string();
        let mut files = BTreeMap::new();
        for (name, bytes, kind) in std::iter::once((
            "manifest.json",
            manifest.as_bytes(),
            tar::EntryType::Regular,
        ))
        .chain(entries.iter().copied())
        {
            let mut header = tar::Header::new_ustar();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(kind);
            if kind.is_symlink() {
                header.set_link_name("/tmp/outside").unwrap();
            }
            let path = format!("package/{name}");
            header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
            header.set_cksum();
            archive.append(&header, bytes).unwrap();
            files.insert(
                name.to_owned(),
                Fingerprint {
                    size: bytes.len() as u64,
                    sha256: hash(bytes),
                },
            );
        }
        archive.into_inner().unwrap().finish().unwrap();
        let bytes = fs::read(&path).unwrap();
        let package = Package {
            schema: 1,
            pack: "extra".into(),
            version: "0.0.0".into(),
            target: TARGET.into(),
            name: "test".into(),
            url: "unused".into(),
            size: bytes.len() as u64,
            sha256: hash(&bytes),
            libraries: vec![],
            files,
        };
        (directory, path, package)
    }

    #[test]
    fn streamed_download_is_bounded_and_rejects_interruption() {
        for (body, content_length, expected_size, succeeds) in [
            ("abcdefgh", 8, 8, true),
            ("abcdefgh", 8, 4, false),
            ("abc", 8, 8, false),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/pack.tgz", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                stream.read(&mut request).unwrap();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n{body}").unwrap();
            });
            let directory = tempfile::tempdir().unwrap();
            let destination = directory.path().join("download");
            let result = tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(download_archive(&url, expected_size, &destination));
            server.join().unwrap();
            assert_eq!(result.is_ok(), succeeds, "{result:?}");
            assert!(destination.metadata().unwrap().len() <= expected_size);
        }
    }

    #[test]
    fn archive_requires_exact_bytes_files_and_target() {
        let (_dir, archive, package) =
            fixture(&[("parser.so", b"parser", tar::EntryType::Regular)]);
        let destination = tempfile::tempdir().unwrap();
        unpack(&package, &archive, destination.path()).unwrap();
        verify_install(&package, destination.path()).unwrap();
        fs::write(destination.path().join("parser.so"), b"broken").unwrap();
        assert!(verify_install(&package, destination.path()).is_err());
        for mutation in 0..4 {
            let mut bad = package.clone();
            match mutation {
                0 => bad.sha256 = "00".repeat(32),
                1 => bad.target = "wrong-target".into(),
                2 => {
                    bad.files.remove("parser.so");
                }
                _ => {
                    bad.files.get_mut("parser.so").unwrap().size = MAX_FILE + 1;
                }
            }
            assert!(unpack(&bad, &archive, tempfile::tempdir().unwrap().path()).is_err());
        }
        let mut bytes = fs::read(&archive).unwrap();
        bytes.truncate(bytes.len() / 2);
        fs::write(&archive, bytes).unwrap();
        assert!(unpack(&package, &archive, tempfile::tempdir().unwrap().path()).is_err());
    }

    #[test]
    fn archives_reject_links_nested_paths_and_duplicates() {
        for entries in [
            vec![("linked", b"".as_slice(), tar::EntryType::Symlink)],
            vec![("hardlink", b"".as_slice(), tar::EntryType::Link)],
            vec![("../escape", b"x".as_slice(), tar::EntryType::Regular)],
            vec![("nested/file", b"x".as_slice(), tar::EntryType::Regular)],
            vec![("duplicate", b"x".as_slice(), tar::EntryType::Regular); 2],
        ] {
            let (_dir, archive, package) = fixture(&entries);
            assert!(unpack(&package, &archive, tempfile::tempdir().unwrap().path()).is_err());
        }
    }
}
