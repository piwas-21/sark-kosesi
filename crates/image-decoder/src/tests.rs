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
            let blocked = mode.starts_with("blocked-");
            if blocked {
                crate::sandbox::install().expect("install sandbox");
            }
            let result: io::Result<()> = if mode.ends_with("open") {
                std::fs::read(path).map(|_| ())
            } else if mode.ends_with("unlink") {
                std::fs::remove_file(path)
            } else if mode.ends_with("socket") {
                std::os::unix::net::UnixStream::connect(path).map(|_| ())
            } else if mode.ends_with("spawn") {
                std::process::Command::new("/bin/true")
                    .status()
                    .and_then(|status| {
                        if status.success() {
                            Ok(())
                        } else {
                            Err(io::Error::other("control child failed"))
                        }
                    })
            } else {
                rustix::process::test_kill_process(pid).map_err(io::Error::from)
            };
            if blocked {
                assert_eq!(
                    result
                        .expect_err("sandbox must deny operation")
                        .raw_os_error(),
                    Some(libc::EPERM),
                    "sandbox must return EPERM"
                );
            } else {
                result.expect("unfiltered control must succeed");
            }
        }
        "term-supervisor" => term_supervisor_probe(),
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
    let (_shutdown_tx, mut shutdown) = watch::channel(false);
    let result = run_child(
        &mut socket,
        child,
        b"",
        b"",
        Duration::from_millis(250),
        &mut shutdown,
    )
    .await;
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
            assert!(
                status.success(),
                "{name} probe must verify its expected outcome"
            );
            if blocked {
                assert!(marker.exists(), "blocked probe must preserve fixture");
            } else if name == "unlink" {
                assert!(!marker.exists());
            }
        }
    }
    std::fs::remove_file(&marker).expect("fixture cleanup");
    std::fs::remove_file(&socket).expect("socket cleanup");
    std::fs::remove_dir(&dir).expect("directory cleanup");
}

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

#[tokio::test]
async fn an_extra_socket_byte_cancels_and_reaps_the_child() {
    let child = command("sleep").spawn().expect("probe child");
    let pid = child.id().expect("child pid");
    let (mut socket, mut peer) = UnixStream::pair().expect("socket pair");
    peer.write_all(b"x").await.expect("extra byte");
    let (_shutdown_tx, mut shutdown) = watch::channel(false);
    let outcome = run_child(
        &mut socket,
        child,
        b"",
        b"",
        Duration::from_secs(3),
        &mut shutdown,
    )
    .await;
    assert!(matches!(outcome, Ok(None)));
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}

fn term_supervisor_probe() {
    let ready_path = std::env::var_os("DECODER_TEST_READY").expect("readiness path");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(async {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            let child = command("sleep").spawn().expect("active child");
            let pid = child.id().expect("child pid");
            let (mut socket, peer) = UnixStream::pair().expect("socket pair");
            let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
            let mut tasks = tokio::task::JoinSet::new();
            tasks.spawn(async move {
                let _peer = peer;
                let result = run_child(
                    &mut socket,
                    child,
                    b"",
                    b"",
                    Duration::from_secs(20),
                    &mut shutdown_rx,
                )
                .await;
                assert!(matches!(result, Ok(None)), "SIGTERM cancels child");
            });
            tokio::task::yield_now().await;
            std::fs::write(ready_path, pid.to_string()).expect("ready marker");
            term.recv().await.expect("SIGTERM received");
            crate::server::shutdown_tasks(&shutdown_tx, &mut tasks)
                .await
                .expect("drain tasks");
            assert!(
                !std::path::Path::new(&format!("/proc/{pid}")).exists(),
                "child reaped before returning"
            );
        });
}

#[tokio::test]
async fn sigterm_cancels_real_child_and_waits_for_reaping() {
    let marker = std::env::temp_dir().join(format!("decoder-term-{}", std::process::id()));
    let mut supervisor = command("term-supervisor")
        .env("DECODER_TEST_READY", &marker)
        .spawn()
        .expect("supervisor probe");
    let child_pid = timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(content) = std::fs::read_to_string(&marker) {
                break content.parse::<u32>().expect("child pid");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("supervisor ready");
    assert!(std::path::Path::new(&format!("/proc/{child_pid}")).exists());
    let pid = rustix::process::Pid::from_raw(
        i32::try_from(supervisor.id().expect("supervisor pid")).expect("pid range"),
    )
    .expect("nonzero pid");
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).expect("send SIGTERM");
    let status = timeout(Duration::from_secs(5), supervisor.wait())
        .await
        .expect("supervisor shutdown timeout")
        .expect("supervisor status");
    assert!(
        status.success(),
        "supervisor must drain its child before exit"
    );
    assert!(!std::path::Path::new(&format!("/proc/{child_pid}")).exists());
    std::fs::remove_file(marker).expect("remove readiness marker");
}
