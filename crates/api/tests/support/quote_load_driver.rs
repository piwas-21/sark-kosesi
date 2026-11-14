use super::*;
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

const STEADY_MS: u64 = 600_000;
const MAX_DURATION: Duration = Duration::from_secs(655);

fn write_private(path: &Path, contents: &[u8]) {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .expect("new private fixture file");
    file.write_all(contents)
        .expect("write private fixture file");
}

#[derive(Serialize)]
struct Credentials<'a> {
    cookie: &'a str,
    csrf: &'a str,
}

#[derive(Serialize)]
struct Fixture<'a> {
    url: String,
    origin: &'static str,
    bearer: &'a str,
    owners: Vec<Credentials<'a>>,
}

fn launch(server: &mut Server, bearer: &str, owners: &[Owner], directory: &Path) -> Child {
    assert!(server.owns_listener(), "owned quote listener required");
    let fixtures = directory.join("quote-owners.json");
    let config = directory.join("quote-k6-config.json");
    let fixture = Fixture {
        url: format!("http://{}", server.address),
        origin: ORIGIN,
        bearer,
        owners: owners
            .iter()
            .map(|owner| Credentials {
                cookie: owner.cookie.as_str(),
                csrf: owner.csrf.as_str(),
            })
            .collect(),
    };
    let payload = Zeroizing::new(serde_json::to_vec(&fixture).expect("serialize k6 fixture"));
    write_private(&fixtures, &payload);
    write_private(&config, b"{}");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let stdout = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(directory.join("quote-k6-summary.json"))
        .expect("private k6 summary");
    Command::new(root.join(".local/bin/k6"))
        .current_dir(&root)
        .args([
            "run",
            "--quiet",
            "--no-color",
            "--no-usage-report",
            "--log-output",
            "none",
            "--log-format",
            "raw",
            "--config",
        ])
        .arg(config)
        .arg("--console-output")
        .arg(directory.join("quote-k6-samples.jsonl"))
        .arg("tests/application/quote-load.js")
        .env_clear()
        .env("HOME", directory)
        .env("TMPDIR", directory)
        .env("K6_NO_USAGE_REPORT", "true")
        .env("QUOTE_LOAD_FIXTURE", fixtures)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::null())
        .spawn()
        .expect("start owned k6 process")
}

fn read_local(path: &Path, maximum: u64) -> String {
    let entry = fs::symlink_metadata(path).expect("fixture evidence file");
    assert!(
        entry.is_file() && entry.len() <= maximum,
        "bounded fixture evidence"
    );
    fs::read_to_string(path).expect("read bounded fixture evidence")
}

fn read_samples(directory: &Path) -> (u64, u64, Vec<Value>) {
    let output = read_local(&directory.join("quote-k6-samples.jsonl"), 2_000_000);
    let mut start = u64::MAX;
    let mut burst_end = 0;
    let mut requests = Vec::new();
    for line in output.lines() {
        let mut record: Value = serde_json::from_str(line).expect("invalid k6 sample JSON");
        let scenario_start = record["scenario_start_ms"]
            .as_u64()
            .expect("scenario clock");
        let request = record["request"].take();
        match request["workload"].as_str() {
            Some("steady") => start = start.min(scenario_start),
            Some("burst") => {
                burst_end = burst_end.max(request["finished_ms"].as_u64().expect("finish clock"));
            }
            _ => panic!("unexpected k6 sample"),
        }
        requests.push(request);
    }
    let summary = read_local(&directory.join("quote-k6-summary.json"), 16_384);
    let summary: Value = serde_json::from_str(summary.lines().last().expect("k6 summary record"))
        .expect("invalid k6 summary JSON");
    assert!(
        start != u64::MAX
            && burst_end != 0
            && summary["requests"].as_u64() == Some(requests.len() as u64)
            && summary["dropped"].as_u64() == Some(0)
            && summary["duration_ms"]
                .as_f64()
                .is_some_and(|ms| (600_000.0..=650_000.0).contains(&ms)),
        "k6 sample totals or duration disagree"
    );
    (start, burst_end, requests)
}

fn ledger(values: (i64, i64, i64, i64, i64, i64, i64)) -> Value {
    json!({
        "stock_holds": values.0, "stock_hold_lines": values.1, "basket_hold_groups": values.2,
        "stock_movements": values.3, "checkout_orders": values.4, "payment_attempts": values.5,
        "commerce_outbox": values.6,
    })
}

async fn require_persisted_ownership(pool: &PgPool, owners: &[Owner], requests: &[Value]) {
    let expected: BTreeSet<_> = requests
        .iter()
        .filter(|sample| sample["status"] == 200)
        .map(|sample| {
            let workload = sample["workload"].as_str().expect("workload");
            let owner = sample["owner"].as_u64().expect("owner") as usize;
            let sequence = sample["sequence"].as_u64().expect("sequence");
            let index = match workload {
                "steady" if owner < 25 => owner,
                "burst" if owner < 50 => 25 + owner,
                _ => panic!("unbound quote observation"),
            };
            (
                owners[index].basket.clone(),
                format!("load-{workload}-{owner}-{sequence}"),
            )
        })
        .collect();
    let actual: Vec<(String, String)> =
        sqlx::query_as("SELECT basket_id,effect_key FROM guest_quote_snapshots LIMIT 3001")
            .fetch_all(pool)
            .await
            .expect("read persisted quote owners");
    assert!(
        actual.len() == expected.len() && actual.into_iter().collect::<BTreeSet<_>>() == expected,
        "persisted quote ownership disagrees"
    );
}

fn rss_kib(pid: u32) -> u64 {
    let output =
        bounded_output(Command::new("/bin/ps").args(["-o", "rss=", "-p", &pid.to_string()]))
            .expect("API process resource sample");
    let value = std::str::from_utf8(&output.stdout).expect("resource sample");
    assert!(
        output.status.success(),
        "API process resource sample failed"
    );
    value.trim().parse().expect("API process RSS")
}

async fn db_bytes(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT pg_database_size(current_database())")
        .fetch_one(pool)
        .await
        .expect("sample isolated database size")
}

pub(super) async fn run(
    server: &mut Server,
    bearer: &str,
    owners: &[Owner],
    pool: &PgPool,
    before: Effects,
) {
    let proof =
        PathBuf::from(std::env::var_os("SARK_QUOTE_OWNED_DB_PROOF").expect("owned proof path"));
    let directory = proof.parent().expect("owned fixture directory");
    let before_bytes = db_bytes(pool).await;
    let mut peak_rss = rss_kib(server.child.id());
    let mut child = launch(server, bearer, owners, directory);
    let deadline = Instant::now() + MAX_DURATION;
    let completed = loop {
        let state = child.try_wait();
        if let Ok(Some(status)) = state {
            break status.success();
        }
        if state.is_err() || Instant::now() >= deadline || !server.owns_listener() {
            let _ = child.kill();
            let _ = child.wait();
            break false;
        }
        peak_rss = peak_rss.max(rss_kib(server.child.id()));
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    fs::remove_file(directory.join("quote-owners.json")).expect("remove fixture credentials");
    assert!(
        completed && server.owns_listener(),
        "owned k6 run did not complete"
    );
    let (start, burst_end, requests) = read_samples(directory);
    require_persisted_ownership(pool, owners, &requests).await;
    let after = effects(pool).await;
    let report = json!({
        "observations": {
            "schemaVersion": 1,
            "steady_start_ms": start,
            "steady_end_ms": start.checked_add(STEADY_MS).expect("steady clock"),
            "burst_end_ms": burst_end,
            "requests": requests,
            "stock_before": before.1,
            "stock_after": after.1,
            "ledgers_before": ledger(before.2),
            "ledgers_after": ledger(after.2),
            "quote_count_before": before.0,
            "quote_count_after": after.0,
        },
        "resources": {
            "api_peak_sampled_rss_kib": peak_rss,
            "database_bytes_before": before_bytes,
            "database_bytes_after": db_bytes(pool).await,
            "rss_sample_seconds": 2,
        },
    });
    write_private(
        &directory.join("quote-observations.json"),
        report.to_string().as_bytes(),
    );
}
