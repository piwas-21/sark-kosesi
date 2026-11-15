//! One admitted request at a time; socket peers never select an executable or command.
use crate::{child::MAX_INPUT, supervise};
use commerce_image_codec::{Rejection, Response, wire};
use std::{
    io,
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::UnixListener as StdListener,
    },
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    signal::unix::{SignalKind, signal},
    sync::Semaphore,
    time::timeout,
};

const IO_DEADLINE: Duration = Duration::from_secs(10);

struct SocketGuard {
    path: PathBuf,
    inode: u64,
    device: u64,
}
impl Drop for SocketGuard {
    fn drop(&mut self) {
        if let Ok(m) = std::fs::symlink_metadata(&self.path)
            && m.file_type().is_socket()
            && m.ino() == self.inode
            && m.dev() == self.device
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn bind(path: &Path) -> io::Result<(UnixListener, SocketGuard)> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("socket parent"))?;
    if !path.is_absolute() || path.file_name().is_none() || parent.canonicalize()? != parent {
        return Err(io::Error::other("socket path"));
    }
    let dir = std::fs::symlink_metadata(parent)?;
    if !dir.is_dir()
        || dir.uid() != rustix::process::getuid().as_raw()
        || dir.mode() & 0o022 != 0
        || rustix::process::getuid().as_raw() == 0
    {
        return Err(io::Error::other("socket directory"));
    }
    // Never unlink a pre-existing object, including an apparently stale socket.
    let listener = StdListener::bind(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    let guard = SocketGuard {
        path: path.to_owned(),
        inode: metadata.ino(),
        device: metadata.dev(),
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))?;
    listener.set_nonblocking(true)?;
    Ok((UnixListener::from_std(listener)?, guard))
}

pub(crate) async fn serve(path: PathBuf, allowed_uid: u32) -> io::Result<()> {
    let mut shutdown = signal(SignalKind::terminate())?;
    let (listener, _guard) = bind(&path)?;
    let capacity = Arc::new(Semaphore::new(1));
    loop {
        let (mut socket, _) = tokio::select! {
            _ = shutdown.recv() => return Ok(()),
            connection = listener.accept() => connection?,
        };
        if !socket
            .peer_cred()
            .is_ok_and(|credentials| credentials.uid() == allowed_uid)
        {
            continue;
        }
        match capacity.clone().try_acquire_owned() {
            Ok(permit) => {
                tokio::spawn(async move {
                    let _permit = permit;
                    let _ = request(socket).await;
                });
            }
            Err(_) => {
                let _ = timeout(IO_DEADLINE, unavailable(&mut socket)).await;
            }
        }
    }
}

async fn unavailable(socket: &mut UnixStream) -> io::Result<()> {
    let bytes = wire::encode_response(&Response::Rejected(Rejection::Unavailable))
        .map_err(|_| io::Error::other("response"))?;
    socket.write_all(&bytes).await
}

async fn request(mut socket: UnixStream) -> io::Result<()> {
    let input = timeout(IO_DEADLINE, async {
        let mut header = [0; wire::HEADER_LEN];
        socket.read_exact(&mut header).await?;
        let (_, len) =
            wire::parse_request_header(&header).map_err(|_| io::Error::other("header"))?;
        if len == 0 || len > MAX_INPUT {
            return Err(io::Error::other("input length"));
        }
        let mut body = vec![0; len];
        socket.read_exact(&mut body).await?;
        Ok::<_, io::Error>((header, body))
    })
    .await;
    let Ok(Ok((header, body))) = input else {
        return timeout(IO_DEADLINE, unavailable(&mut socket))
            .await
            .map_err(|_| io::Error::other("response timeout"))?;
    };
    match supervise::decode(&mut socket, &header, &body).await {
        Ok(Some(response)) => timeout(IO_DEADLINE, socket.write_all(&response))
            .await
            .map_err(|_| io::Error::other("response timeout"))?,
        Ok(None) => Ok(()), // Peer closed its write half or sent an extra byte.
        Err(_) => timeout(IO_DEADLINE, unavailable(&mut socket))
            .await
            .map_err(|_| io::Error::other("response timeout"))?,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static ID: AtomicU64 = AtomicU64::new(0);

    #[tokio::test]
    async fn socket_binding_never_replaces_existing_paths() {
        let base = std::env::temp_dir()
            .canonicalize()
            .expect("temporary directory");
        let dir = base.join(format!(
            "decoder-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).expect("private test directory");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .expect("directory mode");
        let path = dir.join("decoder.sock");
        std::fs::write(&path, b"keep").expect("foreign object");
        assert!(bind(&path).is_err());
        assert_eq!(std::fs::read(&path).expect("object preserved"), b"keep");
        std::fs::remove_file(&path).expect("remove fixture");
        let (_listener, guard) = bind(&path).expect("first listener");
        assert_eq!(
            std::fs::symlink_metadata(&path).expect("socket").mode() & 0o777,
            0o660
        );
        assert!(bind(&path).is_err(), "active listener must not be replaced");
        drop(guard);
        assert!(!path.exists());
        std::fs::remove_dir(&dir).expect("remove fixture directory");
    }
}
