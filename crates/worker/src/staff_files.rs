//! Bounded file input for private local/test staff commands.
use serde::de::DeserializeOwned;
use std::io;

pub(super) enum Visibility {
    Regular,
    OwnerOnly,
}

pub(super) fn read_private_json<T: DeserializeOwned>(path: &str, max_bytes: u64) -> io::Result<T> {
    let unavailable = || io::Error::other("staff input file unavailable");
    let bytes = read(path, max_bytes, Visibility::OwnerOnly)?;
    if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return Err(unavailable());
    }
    serde_json::from_slice(&bytes).map_err(|_| unavailable())
}

#[cfg(unix)]
pub(super) fn read(path: &str, max_bytes: u64, visibility: Visibility) -> io::Result<Vec<u8>> {
    use std::{
        fs,
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let unavailable = || io::Error::other("staff input file unavailable");
    let allowed = |metadata: &fs::Metadata| {
        metadata.is_file()
            && metadata.len() <= max_bytes
            && (matches!(visibility, Visibility::Regular) || metadata.mode() & 0o077 == 0)
    };
    let before = fs::symlink_metadata(path).map_err(|_| unavailable())?;
    if !allowed(&before) {
        return Err(unavailable());
    }
    // A replaced FIFO cannot block open. Reject symlinks and changed identities.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| unavailable())?;
    let opened = file.metadata().map_err(|_| unavailable())?;
    if !allowed(&opened) || (before.dev(), before.ino()) != (opened.dev(), opened.ino()) {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if u64::try_from(bytes.len()).map_err(|_| unavailable())? > max_bytes {
        return Err(unavailable());
    }
    Ok(bytes)
}

#[cfg(not(unix))]
pub(super) fn read(_: &str, _: u64, _: Visibility) -> io::Result<Vec<u8>> {
    Err(io::Error::other("staff input file unavailable"))
}
