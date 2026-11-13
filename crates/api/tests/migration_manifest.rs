//! The CLI reports compiled SQLx metadata without a database or environment.
use serde_json::{Value, json};
use sha2::{Digest, Sha384};
use std::{fs, path::PathBuf, process::Command};

fn expected_migrations() -> Vec<Value> {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
    let mut rows = Vec::new();
    for entry in fs::read_dir(directory).expect("migration directory") {
        let entry = entry.expect("migration entry");
        let name = entry.file_name().into_string().expect("UTF-8 filename");
        if !name.ends_with(".sql") {
            continue;
        }
        let (number, _) = name.split_once('_').expect("versioned migration filename");
        let version: i64 = number.parse().expect("numeric migration version");
        let (kind, order) = if name.ends_with(".up.sql") {
            ("reversible_up", 1)
        } else if name.ends_with(".down.sql") {
            ("reversible_down", 2)
        } else {
            ("simple", 0)
        };
        let bytes = fs::read(entry.path()).expect("migration bytes");
        let checksum = format!("{:x}", Sha384::digest(&bytes));
        rows.push((
            version,
            order,
            json!({
                "version": version.to_string(),
                "kind": kind,
                "checksum_sha384": checksum
            }),
        ));
    }
    rows.sort_by_key(|(version, order, _)| (*version, *order));
    rows.into_iter().map(|(_, _, row)| row).collect()
}

fn agrees(document: &Value, expected: &[Value]) -> bool {
    document
        == &json!({
            "schema_version": 1,
            "ignore_missing": false,
            "migrations": expected,
        })
        && !expected.is_empty()
}

#[test]
fn embedded_manifest_is_exact_closed_and_environment_independent() {
    let expected = expected_migrations();
    assert!(!expected.is_empty());
    assert_eq!(
        expected[0],
        json!({
            "version":"1",
            "kind":"simple",
            "checksum_sha384":"66f189f0586c65fd9986e81a28a7b56e725c25ec394c94efcbd98d0aa77cd21246a3a564a884338da185392e7c796fc2"
        })
    );
    let run = |poison: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_commerce-api"));
        command
            .arg("migration-manifest")
            .env_clear()
            .current_dir(std::env::temp_dir());
        if poison {
            command
                .env("APP_ENV", "production")
                .env("DATABASE_URL", "not-a-database-url")
                .env("TOKIO_WORKER_THREADS", "not-a-number")
                .env("PREVIEW_TOKEN", "synthetic-secret-never-print");
        }
        let output = command.output().expect("run manifest command");
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-secret-never-print"));
        serde_json::from_slice::<Value>(&output.stdout).expect("one JSON document")
    };
    let clean = run(false);
    assert!(
        agrees(&clean, &expected),
        "compiled manifest differs from source bytes"
    );
    assert_eq!(run(true), clean);

    // Controls establish that an empty, incomplete, duplicate, wrong-type or
    // altered digest result cannot satisfy the independent comparison.
    let mut malformed = clean.clone();
    malformed["migrations"] = json!([]);
    assert!(!agrees(&malformed, &expected));
    malformed = clean.clone();
    malformed["migrations"].as_array_mut().expect("array").pop();
    assert!(!agrees(&malformed, &expected));
    malformed = clean.clone();
    malformed["migrations"]
        .as_array_mut()
        .expect("array")
        .push(expected[0].clone());
    assert!(!agrees(&malformed, &expected));
    malformed = clean.clone();
    malformed["migrations"][0]["kind"] = json!("reversible_up");
    assert!(!agrees(&malformed, &expected));
    malformed = clean.clone();
    malformed["migrations"][0]["version"] = json!("999");
    assert!(!agrees(&malformed, &expected));
    malformed = clean.clone();
    malformed["migrations"][0]["checksum_sha384"] = json!("0".repeat(96));
    assert!(!agrees(&malformed, &expected));
    malformed = clean.clone();
    malformed["ignore_missing"] = json!(true);
    assert!(!agrees(&malformed, &expected));
    malformed = clean.clone();
    malformed["unexpected"] = json!(true);
    assert!(!agrees(&malformed, &expected));

    let extra = Command::new(env!("CARGO_BIN_EXE_commerce-api"))
        .args(["migration-manifest", "extra"])
        .env_clear()
        .output()
        .expect("run invalid command");
    assert!(!extra.status.success());
    assert!(extra.stdout.is_empty());
}

#[test]
fn existing_openapi_and_server_startup_dispatch_are_preserved() {
    let openapi = Command::new(env!("CARGO_BIN_EXE_commerce-api"))
        .args(["openapi", "previously-ignored"])
        .env_clear()
        .env("TOKIO_WORKER_THREADS", "not-a-number")
        .output()
        .expect("run openapi command");
    assert!(openapi.status.success());
    assert_eq!(
        openapi.stdout,
        fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../packages/contracts/openapi.json")
        )
        .expect("committed contract")
    );
    let server = Command::new(env!("CARGO_BIN_EXE_commerce-api"))
        .env_clear()
        .env("APP_ENV", "production")
        .env("TOKIO_WORKER_THREADS", "1")
        .output()
        .expect("run server entry");
    assert!(!server.status.success());
    assert!(String::from_utf8_lossy(&server.stderr).contains("production is disabled"));
}
