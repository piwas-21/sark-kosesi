//! Bounded bearer input for private staff commands.
use commerce_infrastructure::staff_access::SessionToken;
use std::io;
#[cfg(unix)]
use std::{
    io::{IsTerminal, Read},
    os::fd::AsFd,
    sync::mpsc,
    time::Duration,
};
#[cfg(unix)]
use zeroize::Zeroizing;

#[cfg(unix)]
const INPUT_DEADLINE: Duration = Duration::from_secs(5);

/// Exactly 32 raw bytes followed by EOF on a non-terminal stdin.
/// A detached standard thread permits a blocked pipe to time out without
/// making Tokio wait for a blocking task during runtime shutdown.
#[cfg(unix)]
pub(super) fn read_token() -> io::Result<SessionToken> {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return Err(io::Error::other("staff review input unavailable"));
    }
    // File reads the owned duplicate directly, without Stdin's shared buffer.
    let fd = stdin
        .as_fd()
        .try_clone_to_owned()
        .map_err(|_| io::Error::other("staff review input unavailable"))?;
    let input = std::fs::File::from(fd);
    let (send, recv) = mpsc::sync_channel(1);
    let _reader = std::thread::Builder::new()
        .spawn(move || {
            let mut bytes = Zeroizing::new(Vec::with_capacity(33));
            let read = input.take(33).read_to_end(&mut bytes);
            let _ = send.send(read.map(|_| bytes));
        })
        .map_err(|_| io::Error::other("staff review input unavailable"))?;
    let bytes = recv
        .recv_timeout(INPUT_DEADLINE)
        .map_err(|_| io::Error::other("staff review input unavailable"))?
        .map_err(|_| io::Error::other("staff review input unavailable"))?;
    if bytes.len() != 32 {
        return Err(io::Error::other("staff review input unavailable"));
    }
    let mut token = Zeroizing::new([0_u8; 32]);
    token.copy_from_slice(&bytes);
    Ok(SessionToken::from_bytes(*token))
}

#[cfg(not(unix))]
pub(super) fn read_token() -> io::Result<SessionToken> {
    Err(io::Error::other("staff review input unavailable"))
}
