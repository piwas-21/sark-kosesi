//! Actual API startup against isolated, synthetic PostgreSQL migration history.
use commerce_api::config::Config;
use commerce_infrastructure::MIGRATOR;
use sqlx::{ConnectOptions, Executor, PgPool};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const TOKEN: &str = "synthetic-preview-token-1234567890";

#[test]
fn preview_decoder_requires_a_canonical_unix_socket_path() {
    let mut values = HashMap::from([
        ("APP_ENV".into(), "staging".into()),
        (
            "DATABASE_URL".into(),
            "postgresql://localhost/isolated".into(),
        ),
        ("PREVIEW_TOKEN".into(), TOKEN.into()),
        ("API_BIND".into(), "127.0.0.1:4567".into()),
    ]);
    assert!(
        Config::parse(&values)
            .expect("default")
            .image_decoder
            .is_none()
    );
    values.insert(
        "PREVIEW_IMAGE_DECODER_SOCKET".into(),
        "/decoder-socket/decoder.sock".into(),
    );
    assert!(
        Config::parse(&values)
            .expect("configured")
            .image_decoder
            .is_some()
    );
    for path in [
        "",
        "relative.sock",
        "/decoder-socket/../decoder.sock",
        "/double//path",
        "/invalid path",
    ] {
        values.insert("PREVIEW_IMAGE_DECODER_SOCKET".into(), path.into());
        assert!(Config::parse(&values).is_err(), "{path}");
    }
}

#[test]
fn preapplied_configuration_is_explicit_staging_only_and_excludes_writers() {
    let mut values = HashMap::from([
        ("APP_ENV".into(), "staging".into()),
        (
            "DATABASE_URL".into(),
            "postgresql://localhost/isolated".into(),
        ),
        ("PREVIEW_TOKEN".into(), TOKEN.into()),
        ("API_BIND".into(), "127.0.0.1:4567".into()),
    ]);
    assert!(
        !Config::parse(&values)
            .expect("default")
            .preview_database_preapplied
    );
    values.insert("PREVIEW_DATABASE_PREAPPLIED".into(), "0".into());
    assert!(
        !Config::parse(&values)
            .expect("off")
            .preview_database_preapplied
    );
    values.insert("PREVIEW_DATABASE_PREAPPLIED".into(), "1".into());
    assert!(
        Config::parse(&values)
            .expect("staging")
            .preview_database_preapplied
    );
    for (key, value) in [
        ("GUEST_ORIGIN", "https://example.test"),
        ("CALLBACK_INTAKE_PROFILE_ID", "pfl_SYNTHETIC_TestOnly"),
        ("TEST_LOGIN_ORIGIN", "https://example.test"),
        ("TEST_VERIFICATION_ORIGIN", "https://example.test"),
        ("TEST_LOGIN_CSRF_TRANSPORT", "1"),
        ("TEST_FAVOURITES_TRANSPORT", "1"),
        ("TEST_FAVOURITES_MUTATION_TRANSPORT", "1"),
        ("TEST_LINKED_ORDERS_TRANSPORT", "1"),
        ("TEST_RESTOCK_TRANSPORT", "1"),
        ("TEST_RESTOCK_MUTATION_TRANSPORT", "1"),
    ] {
        let mut denied = values.clone();
        denied.insert(key.into(), value.into());
        assert!(Config::parse(&denied).is_err(), "{key}");
    }
    for environment in ["local", "test", "production", ""] {
        let mut denied = values.clone();
        denied.insert("APP_ENV".into(), environment.into());
        assert!(Config::parse(&denied).is_err(), "{environment}");
    }
    for environment in ["local", "test", "staging"] {
        let mut off = values.clone();
        off.insert("APP_ENV".into(), environment.into());
        off.insert("PREVIEW_DATABASE_PREAPPLIED".into(), "0".into());
        assert!(
            !Config::parse(&off)
                .expect("ordinary startup")
                .preview_database_preapplied
        );
    }
    for flag in ["", "true", "false", "2", " 1", "1 "] {
        let mut denied = values.clone();
        denied.insert("PREVIEW_DATABASE_PREAPPLIED".into(), flag.into());
        assert!(Config::parse(&denied).is_err(), "{flag}");
    }
}

struct Process(Child);
impl Process {
    fn shutdown(&mut self) {
        let status = Command::new("/bin/kill")
            .args(["-s", "INT", &self.0.id().to_string()])
            .status()
            .expect("signal API");
        assert!(status.success(), "signal API");
        assert!(
            self.0.wait().expect("stop API").success(),
            "graceful shutdown"
        );
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn start(pool: &PgPool, options: &str, decoder: Option<&str>) -> (Process, std::net::SocketAddr) {
    let reserved = TcpListener::bind("127.0.0.1:0").expect("port");
    let address = reserved.local_addr().expect("address");
    drop(reserved);
    let mut command = Command::new(env!("CARGO_BIN_EXE_commerce-api"));
    command.env_clear();
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    if let Some(socket) = decoder {
        command.env("PREVIEW_IMAGE_DECODER_SOCKET", socket);
    }
    let child = command
        .env("APP_ENV", "staging")
        .env("PREVIEW_DATABASE_PREAPPLIED", "1")
        .env("PREVIEW_TOKEN", TOKEN)
        .env("API_BIND", address.to_string())
        .env(
            "DATABASE_URL",
            pool.connect_options().to_url_lossy().to_string(),
        )
        .env("PGOPTIONS", options)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start API");
    (Process(child), address)
}
fn ready(process: &mut Process, address: std::net::SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            process.0.try_wait().expect("status").is_none(),
            "API exited before readiness"
        );
        if let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("timeout");
            stream
                .write_all(
                    b"GET /health/ready HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                )
                .expect("request");
            let mut response = String::new();
            stream
                .take(8193)
                .read_to_string(&mut response)
                .expect("response");
            assert!(
                response.starts_with("HTTP/1.1 204"),
                "unexpected readiness response"
            );
            return;
        }
        assert!(Instant::now() < deadline, "API did not listen");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn rejected(process: &mut Process, address: std::net::SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(
            TcpStream::connect_timeout(&address, Duration::from_millis(20)).is_err(),
            "invalid history opened listener"
        );
        if let Some(status) = process.0.try_wait().expect("status") {
            assert!(!status.success(), "invalid history accepted");
            return;
        }
        assert!(Instant::now() < deadline, "invalid history did not fail");
        std::thread::sleep(Duration::from_millis(20));
    }
}
async fn history(pool: &PgPool) -> String {
    sqlx::query_scalar(
        "SELECT json_agg(row_to_json(m) ORDER BY version)::text FROM public._sqlx_migrations m",
    )
    .fetch_one(pool)
    .await
    .expect("history")
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn preapplied_binary_validates_complete_history_without_writing(pool: PgPool) {
    pool.execute("CREATE SCHEMA shadow; CREATE TABLE shadow._sqlx_migrations (version bigint, success bool, checksum bytea); CREATE TABLE shadow.catalogue_items (bad integer)")
        .await.expect("shadow fixtures");
    let options = "-c default_transaction_read_only=on -c search_path=shadow";
    let before = history(&pool).await;
    let (mut process, address) = start(&pool, options, Some("/decoder-socket/decoder.sock"));
    ready(&mut process, address);
    process.shutdown();
    drop(process);
    assert_eq!(
        history(&pool).await,
        before,
        "startup changed migration history"
    );

    for (change, restore) in [
        (
            "UPDATE public._sqlx_migrations SET success=false WHERE version=1",
            "UPDATE public._sqlx_migrations SET success=true WHERE version=1",
        ),
        (
            "UPDATE public._sqlx_migrations SET checksum=decode(repeat('00',48),'hex') WHERE version=1",
            "",
        ),
        (
            "UPDATE public._sqlx_migrations SET checksum=decode(repeat('00',47),'hex') WHERE version=1",
            "",
        ),
        (
            "UPDATE public._sqlx_migrations SET checksum=decode(repeat('00',1048576),'hex') WHERE version=1",
            "",
        ),
        ("DELETE FROM public._sqlx_migrations WHERE version=66", ""),
        (
            "INSERT INTO public._sqlx_migrations(version,description,success,checksum,execution_time) VALUES (999999,'unknown',true,decode(repeat('00',48),'hex'),0)",
            "DELETE FROM public._sqlx_migrations WHERE version=999999",
        ),
        (
            "DELETE FROM public._sqlx_migrations WHERE version=66; INSERT INTO public._sqlx_migrations(version,description,success,checksum,execution_time) VALUES (999999,'unknown',true,decode(repeat('00',48),'hex'),0)",
            "DELETE FROM public._sqlx_migrations WHERE version=999999; INSERT INTO public._sqlx_migrations SELECT * FROM public.history_backup WHERE version=66",
        ),
        (
            "INSERT INTO public._sqlx_migrations(version,description,success,checksum,execution_time) SELECT 1000000+n,'overflow',true,decode(repeat('00',48),'hex'),0 FROM generate_series(1,100) n",
            "DELETE FROM public._sqlx_migrations WHERE version BETWEEN 1000001 AND 1000100",
        ),
    ] {
        // Restore the exact synthetic record after each corruption, not by running migrations.
        pool.execute("CREATE TABLE public.history_backup AS SELECT * FROM public._sqlx_migrations WHERE version IN (1,66)").await.expect("backup");
        pool.execute(change).await.expect("corrupt history");
        let corrupt = history(&pool).await;
        let (mut process, address) = start(&pool, options, None);
        rejected(&mut process, address);
        assert_eq!(history(&pool).await, corrupt, "failure changed history");
        if restore.is_empty() {
            pool.execute("DELETE FROM public._sqlx_migrations WHERE version IN (1,66); INSERT INTO public._sqlx_migrations SELECT * FROM public.history_backup").await.expect("restore");
        } else {
            pool.execute(restore).await.expect("restore");
        }
        pool.execute("DROP TABLE public.history_backup")
            .await
            .expect("drop backup");
        assert_eq!(history(&pool).await, before, "synthetic restoration");
    }
    pool.execute("ALTER TABLE public._sqlx_migrations RENAME TO saved_migrations")
        .await
        .expect("hide history");
    let (mut process, address) = start(&pool, options, None);
    rejected(&mut process, address);
    pool.execute("ALTER TABLE public.saved_migrations RENAME TO _sqlx_migrations")
        .await
        .expect("restore history");
    assert_eq!(history(&pool).await, before);

    pool.execute("CREATE TABLE public.history_backup AS SELECT * FROM public._sqlx_migrations WHERE version=66; ALTER TABLE public._sqlx_migrations DROP CONSTRAINT _sqlx_migrations_pkey; DELETE FROM public._sqlx_migrations WHERE version=66; INSERT INTO public._sqlx_migrations SELECT * FROM public._sqlx_migrations WHERE version=1")
        .await.expect("duplicate version");
    let (mut process, address) = start(&pool, options, None);
    rejected(&mut process, address);
    pool.execute("DELETE FROM public._sqlx_migrations WHERE version=1 AND ctid NOT IN (SELECT min(ctid) FROM public._sqlx_migrations WHERE version=1); INSERT INTO public._sqlx_migrations SELECT * FROM public.history_backup; ALTER TABLE public._sqlx_migrations ADD PRIMARY KEY (version); DROP TABLE public.history_backup")
        .await.expect("restore unique versions");
    assert_eq!(history(&pool).await, before);

    pool.execute("ALTER TABLE public._sqlx_migrations ALTER COLUMN checksum DROP NOT NULL; UPDATE public._sqlx_migrations SET checksum=NULL WHERE version=1")
        .await.expect("null checksum");
    let (mut process, address) = start(&pool, options, None);
    rejected(&mut process, address);
    pool.execute("UPDATE public._sqlx_migrations SET checksum=decode('66f189f0586c65fd9986e81a28a7b56e725c25ec394c94efcbd98d0aa77cd21246a3a564a884338da185392e7c796fc2','hex') WHERE version=1; ALTER TABLE public._sqlx_migrations ALTER COLUMN checksum SET NOT NULL")
        .await.expect("restore checksum");
    assert_eq!(history(&pool).await, before);

    let mut blocker = pool.begin().await.expect("lock transaction");
    sqlx::query("LOCK TABLE public._sqlx_migrations IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .expect("block history");
    let started = Instant::now();
    let (mut process, address) = start(&pool, options, None);
    rejected(&mut process, address);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "locked history must fail promptly"
    );
    blocker.rollback().await.expect("release history");
    let (mut process, address) = start(&pool, options, None);
    ready(&mut process, address);
    process.shutdown();
    drop(process);
    assert_eq!(history(&pool).await, before, "recovery changed history");
}
