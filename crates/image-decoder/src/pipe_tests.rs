//! Verify the ordinary supervisor path with an EOF-dependent child process.
use super::*;

#[tokio::test]
async fn complete_request_closes_child_stdin_before_waiting_for_output() {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "cat >/dev/null && printf 'ICR1\\000\\000\\000\\000'"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let child = command.spawn().expect("EOF-dependent child");
    let (mut socket, _peer) = UnixStream::pair().expect("socket pair");
    let (_sender, mut shutdown) = tokio::sync::watch::channel(false);
    let response = run_child(
        &mut socket,
        child,
        b"request",
        b"payload",
        Duration::from_secs(3),
        &mut shutdown,
    )
    .await
    .expect("child exit and reply after its pipe closes");
    assert_eq!(response.as_deref(), Some(b"ICR1\0\0\0\0".as_slice()));
}
