//! Bounded, closed payload input for the local/test packing assertion command.
use commerce_infrastructure::packing_custody::{Component, Units};
use serde::Deserialize;
use std::io;

// Covers the bounded 20 SKU lines and 32 x 64 component entries, including JSON syntax.
#[cfg(unix)]
const MAX_BYTES: u64 = 256 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PackingInput {
    pub units: Vec<Units>,
    pub packages: Vec<Vec<Component>>,
}

#[cfg(unix)]
pub(super) fn read(path: &str) -> io::Result<PackingInput> {
    use std::{
        fs,
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let unavailable = || io::Error::other("packing input unavailable");
    let before = fs::symlink_metadata(path).map_err(|_| unavailable())?;
    if !before.is_file() || before.len() > MAX_BYTES {
        return Err(unavailable());
    }
    // A replaced FIFO cannot block the open; symlinks fail closed. Verify the
    // opened descriptor still refers to the same regular file as the precheck.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| unavailable())?;
    let opened = file.metadata().map_err(|_| unavailable())?;
    if !opened.is_file()
        || opened.len() > MAX_BYTES
        || (before.dev(), before.ino()) != (opened.dev(), opened.ino())
    {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if u64::try_from(bytes.len()).map_err(|_| unavailable())? > MAX_BYTES {
        return Err(unavailable());
    }
    let input: PackingInput = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if !(1..=20).contains(&input.units.len())
        || !(1..=32).contains(&input.packages.len())
        || input
            .packages
            .iter()
            .any(|package| !(1..=64).contains(&package.len()))
    {
        return Err(unavailable());
    }
    Ok(input)
}

#[cfg(not(unix))]
pub(super) fn read(_: &str) -> io::Result<PackingInput> {
    Err(io::Error::other("packing input unavailable"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::symlink,
        sync::atomic::{AtomicU64, Ordering},
    };

    #[test]
    fn accepts_only_bounded_regular_closed_payloads() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "packing-input-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).expect("isolated input directory");
        let file = dir.join("payload.json");
        let file_str = file.to_str().expect("test path");
        let good = br#"{"units":[{"sku":"SET-FILLED","units":1}],"packages":[[{"code":"SEAT-COVER","quantity":2}]]}"#;
        fs::write(&file, good).expect("synthetic file");
        assert!(read(file_str).is_ok());
        for bad in [
            br#"{"units":[],"packages":[]}"#.as_slice(),
            br#"{"units":[{"sku":"A","units":1,"other":0}],"packages":[[{"code":"A","quantity":1}]]}"#,
            br#"{"units":[{"sku":"A","units":1}],"packages":[[{"code":"A","quantity":1}]],"extra":"private-marker"}"#,
            br#"{"units":[],"units":[],"packages":[]}"#,
            b"not JSON",
        ] {
            fs::write(&file, bad).expect("synthetic invalid file");
            assert!(read(file_str).is_err());
        }
        fs::write(
            &file,
            vec![b'x'; usize::try_from(MAX_BYTES + 1).expect("limit")],
        )
        .expect("oversize file");
        assert!(read(file_str).is_err());
        for invalid in [
            serde_json::json!({"units": vec![serde_json::json!({"sku":"A","units":1});21], "packages": [[{"code":"A","quantity":1}]]}),
            serde_json::json!({"units": [{"sku":"A","units":1}], "packages": vec![vec![serde_json::json!({"code":"A","quantity":1})];33]}),
            serde_json::json!({"units": [{"sku":"A","units":1}], "packages": [vec![serde_json::json!({"code":"A","quantity":1});65]]}),
        ] {
            fs::write(&file, serde_json::to_vec(&invalid).expect("fixture JSON"))
                .expect("invalid fixture");
            assert!(read(file_str).is_err());
        }
        fs::write(&file, good).expect("restore valid file");
        let link = dir.join("link");
        symlink(&file, &link).expect("symlink");
        assert!(read(link.to_str().expect("test path")).is_err());
        assert!(read(dir.to_str().expect("test path")).is_err());
        let fifo = dir.join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo")
                .success()
        );
        assert!(read(fifo.to_str().expect("test path")).is_err());
        fs::remove_file(fifo).expect("remove fifo");
        fs::remove_file(link).expect("remove link");
        fs::remove_file(file).expect("remove fixture");
        fs::remove_dir(dir).expect("remove directory");
    }
}
