//! Own each child through termination and reaping, including cancellation and timeout.
use commerce_image_codec::wire;
use std::{io, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    process::{Child, Command},
    sync::watch,
    time::timeout,
};

// Hard CPU cap is 10 s; the wall limit also covers I/O and scheduler stalls.
const WALL_LIMIT: Duration = Duration::from_secs(20);

pub(crate) async fn decode(
    socket: &mut UnixStream,
    header: &[u8; wire::HEADER_LEN],
    body: &[u8],
    shutdown: &mut watch::Receiver<bool>,
) -> io::Result<Option<Vec<u8>>> {
    if *shutdown.borrow() {
        return Ok(None);
    }
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("decode")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let child = command.spawn()?;
    run_child(socket, child, header, body, WALL_LIMIT, shutdown).await
}

async fn run_child(
    socket: &mut UnixStream,
    mut child: Child,
    header: &[u8],
    body: &[u8],
    deadline: Duration,
    shutdown: &mut watch::Receiver<bool>,
) -> io::Result<Option<Vec<u8>>> {
    let Some(mut input) = child.stdin.take() else {
        terminate(&mut child).await;
        return Err(io::Error::other("stdin"));
    };
    let Some(output) = child.stdout.take() else {
        terminate(&mut child).await;
        return Err(io::Error::other("stdout"));
    };
    let result = timeout(deadline, async {
        tokio::select! {
            biased;
            _ = cancelled(shutdown) => Ok(None),
            // Prefer an already-readable disconnect or extra request byte to a ready result.
            _ = socket.read_u8() => Ok(None),
            result = async {
                let writer = async {
                    input.write_all(header).await?;
                    input.write_all(body).await?;
                    input.shutdown().await
                };
                let reader = async {
                    let mut bytes = Vec::new();
                    output.take((wire::MAX_RESPONSE_BYTES + 1) as u64).read_to_end(&mut bytes).await?;
                    if bytes.len() > wire::MAX_RESPONSE_BYTES { return Err(io::Error::other("output length")); }
                    Ok::<_, io::Error>(bytes)
                };
                let (written, read) = tokio::join!(writer, reader);
                written?;
                let bytes = read?;
                if !child.wait().await?.success() { return Err(io::Error::other("child failed")); }
                wire::parse_response(&bytes).map_err(|_| io::Error::other("response"))?;
                Ok(Some(bytes))
            } => result,
        }
    }).await;
    match result {
        Ok(Ok(Some(bytes))) => Ok(Some(bytes)),
        Ok(Ok(None)) => {
            terminate(&mut child).await;
            Ok(None)
        }
        Ok(Err(error)) => {
            terminate(&mut child).await;
            Err(error)
        }
        Err(_) => {
            terminate(&mut child).await;
            Err(io::Error::other("child timeout"))
        }
    }
}

pub(crate) async fn cancelled(shutdown: &mut watch::Receiver<bool>) {
    let stopped = *shutdown.borrow();
    if !stopped {
        let _ = shutdown.changed().await;
    }
}

async fn terminate(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
