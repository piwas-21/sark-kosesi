//! These probes are compiled only into the test harness, never the daemon binary.
use super::*;
use std::os::unix::process::ExitStatusExt;

#[test]
fn probe() {
    let Ok(mode) = std::env::var("DECODER_TEST_PROBE") else {
        return;
    };
    match mode.as_str() {
        "cpu" => {
            crate::child::limits(256 * 1024 * 1024, 1).expect("install CPU limit");
            loop {
                std::hint::black_box(1_u64.wrapping_mul(2));
            }
        }
        "memory" => {
            crate::child::limits(128 * 1024 * 1024, 5).expect("install memory limit");
            assert!(
                Vec::<u8>::new()
                    .try_reserve_exact(512 * 1024 * 1024)
                    .is_err()
            );
        }
        "blocked-open" | "control-open" | "blocked-unlink" | "control-unlink"
        | "blocked-socket" | "control-socket" | "blocked-spawn" | "control-spawn"
        | "blocked-signal" | "control-signal" => {
            let pid = rustix::process::getpid();
            let path = std::env::var_os("DECODER_TEST_PATH").expect("probe path");
            if mode.starts_with("blocked-") {
                crate::sandbox::install().expect("install sandbox");
            }
            if mode.ends_with("open") {
                let _ = std::fs::read(path).expect("read control");
            } else if mode.ends_with("unlink") {
                std::fs::remove_file(path).expect("unlink control");
            } else if mode.ends_with("socket") {
                let _ = std::os::unix::net::UnixStream::connect(path).expect("socket control");
            } else if mode.ends_with("spawn") {
                assert!(
                    std::process::Command::new("/bin/true")
                        .status()
                        .expect("spawn control")
                        .success()
                );
            } else {
                rustix::process::test_kill_process(pid).expect("signal control");
            }
        }
        "sleep" => std::thread::sleep(Duration::from_secs(30)),
        "crash" => std::process::exit(23),
        _ => panic!("unknown test probe"),
    }
}

fn command(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args(["--exact", "supervise::tests::probe", "--nocapture"])
        .env_clear()
        .env("DECODER_TEST_PROBE", mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

#[tokio::test]
async fn actual_cpu_and_address_space_limits_are_enforced() {
    let mut cpu = command("cpu").spawn().expect("cpu probe");
    let status = timeout(Duration::from_secs(8), cpu.wait())
        .await
        .expect("CPU limit deadline")
        .expect("cpu exit");
    assert_eq!(
        status.signal(),
        Some(9),
        "CPU hard limit must kill the process"
    );
    let mut memory = command("memory").spawn().expect("memory probe");
    let status = timeout(Duration::from_secs(5), memory.wait())
        .await
        .expect("memory deadline")
        .expect("memory exit");
    assert!(
        status.success(),
        "allocation must fail inside the hard address-space limit"
    );
}

async fn exercise(mode: &str, cancel: bool) -> (io::Result<Option<Vec<u8>>>, u32) {
    let mut cmd = command(mode);
    let child = cmd.spawn().expect("probe child");
    let pid = child.id().expect("child pid");
    let (mut socket, peer) = UnixStream::pair().expect("socket pair");
    if cancel {
        drop(peer);
    } else {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            drop(peer);
        });
    }
    let result = run_child(&mut socket, child, b"", b"", Duration::from_millis(250)).await;
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "child must be reaped"
    );
    (result, pid)
}

#[tokio::test]
async fn timeout_and_disconnect_kill_and_reap_real_children() {
    let (timed_out, _) = exercise("sleep", false).await;
    assert!(timed_out.is_err());
    let (cancelled, _) = exercise("sleep", true).await;
    assert!(matches!(cancelled, Ok(None)));
    let (crashed, _) = exercise("crash", false).await;
    assert!(crashed.is_err());
}

#[test]
fn seccomp_rejects_real_privileged_operations_with_working_controls() {
    use std::os::unix::net::UnixListener;
    let dir = std::env::temp_dir().join(format!("decoder-probes-{}", std::process::id()));
    std::fs::create_dir(&dir).expect("test directory");
    let marker = dir.join("marker");
    let socket = dir.join("socket");
    let _listener = UnixListener::bind(&socket).expect("test socket");
    for name in ["open", "unlink", "socket", "spawn", "signal"] {
        for blocked in [false, true] {
            std::fs::write(&marker, b"owned fixture").expect("fixture");
            let path = if name == "socket" { &socket } else { &marker };
            let mode = format!("{}-{name}", if blocked { "blocked" } else { "control" });
            let status =
                std::process::Command::new(std::env::current_exe().expect("test executable"))
                    .args(["--exact", "supervise::tests::probe", "--nocapture"])
                    .env_clear()
                    .env("DECODER_TEST_PROBE", mode)
                    .env("DECODER_TEST_PATH", path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .expect("probe status");
            if blocked {
                assert_eq!(
                    status.signal(),
                    Some(31),
                    "{name} must terminate with SIGSYS"
                );
                assert!(marker.exists(), "blocked probe must preserve fixture");
            } else {
                assert!(status.success(), "{name} control must execute successfully");
                if name == "unlink" {
                    assert!(!marker.exists());
                }
            }
        }
    }
    std::fs::remove_file(&marker).expect("fixture cleanup");
    std::fs::remove_file(&socket).expect("socket cleanup");
    std::fs::remove_dir(&dir).expect("directory cleanup");
}

#[tokio::test]
async fn an_extra_socket_byte_cancels_and_reaps_the_child() {
    let child = command("sleep").spawn().expect("probe child");
    let pid = child.id().expect("child pid");
    let (mut socket, mut peer) = UnixStream::pair().expect("socket pair");
    peer.write_all(b"x").await.expect("extra byte");
    let outcome = run_child(&mut socket, child, b"", b"", Duration::from_secs(3)).await;
    assert!(matches!(outcome, Ok(None)));
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}
