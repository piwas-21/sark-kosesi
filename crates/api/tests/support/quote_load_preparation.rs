//! Prepare distinct synthetic load owners in memory; this is not a load run.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};
use tower_sessions::{
    cookie::{Cookie, SameSite},
    session::Id,
};
use zeroize::{Zeroize, Zeroizing};

#[path = "quote_load_driver.rs"]
mod driver;

const OWNERS: usize = 75;
const PER_MINUTE: i32 = 64;
const BASKET: &str = r#"{"expected_revision":0,"effect_key":"load-prepare","lines":[{"sku":"SET-FILLED","quantity":3}]}"#;

// Never format or serialize these credentials. They live only in this process.
struct Owner {
    basket: String,
    cookie: Zeroizing<String>,
    csrf: Zeroizing<String>,
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("fixture clock")
        .as_secs()
}

fn cookie_and_digest(headers: &[(String, String)]) -> (Zeroizing<String>, [u8; 32]) {
    let values: Vec<_> = headers
        .iter()
        .filter(|(name, _)| name == "set-cookie")
        .collect();
    assert!(values.len() == 1, "one guest cookie required");
    let header = &values[0].1;
    let parsed = Cookie::parse(header.clone()).unwrap_or_else(|_| panic!("invalid guest cookie"));
    assert!(
        parsed.name() == "__Host-piwas-guest"
            && parsed.secure() == Some(true)
            && parsed.http_only() == Some(true)
            && parsed.same_site() == Some(SameSite::Lax)
            && parsed.path() == Some("/")
            && parsed.domain().is_none(),
        "invalid guest cookie attributes"
    );
    let pair = header.split(';').next().expect("cookie pair");
    let value = pair
        .strip_prefix("__Host-piwas-guest=")
        .expect("guest cookie name");
    assert!(
        value.len() == 22
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "invalid guest cookie"
    );
    let mut id: Id = value
        .parse()
        .unwrap_or_else(|_| panic!("invalid guest cookie encoding"));
    let mut digest = Sha256::new();
    digest.update(b"piwas:guest-session:v1:");
    let decoded = Zeroizing::new(id.0.to_le_bytes());
    id.0.zeroize();
    digest.update(decoded.as_ref());
    (Zeroizing::new(pair.to_owned()), digest.finalize().into())
}

#[test]
fn cookie_binding_accepts_only_secure_exact_session_identity() {
    let id = Id(12345).to_string();
    let valid = format!("__Host-piwas-guest={id}; HttpOnly; SameSite=Lax; Secure; Path=/");
    let (_, digest) = cookie_and_digest(&[("set-cookie".into(), valid.clone())]);
    assert_eq!(
        digest,
        [
            0x6b, 0x6e, 0xc9, 0x4d, 0x10, 0xef, 0x69, 0x49, 0x37, 0xc0, 0x2a, 0x1b, 0x67, 0xb2,
            0x48, 0xa8, 0x67, 0x67, 0x7a, 0x4b, 0x65, 0xcf, 0xeb, 0x78, 0xc3, 0x3f, 0xba, 0x54,
            0x7b, 0x59, 0xda, 0xfb
        ]
    );
    for wrong in [
        valid.replace("; Secure", ""),
        valid.replace("; HttpOnly", ""),
        valid.replace("SameSite=Lax", "SameSite=None"),
        valid.replace("Path=/", "Path=/bad"),
        format!("{valid}; Domain=example.test"),
        "__Host-piwas-guest=private-marker; HttpOnly; SameSite=Lax; Secure; Path=/".into(),
        "__Host-piwas-guest=AAAAAAAAAAAAAAAAAAAAAZ; HttpOnly; SameSite=Lax; Secure; Path=/".into(),
    ] {
        let outcome =
            std::panic::catch_unwind(|| cookie_and_digest(&[("set-cookie".into(), wrong)]));
        assert!(outcome.is_err(), "malformed cookie accepted");
    }
    assert!(
        std::panic::catch_unwind(|| cookie_and_digest(&[
            ("set-cookie".into(), valid.clone()),
            ("set-cookie".into(), valid)
        ]))
        .is_err(),
        "duplicate cookie accepted"
    );
}

// Observe the real quota while carrying clock custody across the whole preparation.
fn observe_time(previous: &mut (i64, i64), now: i64) -> bool {
    if now < previous.0 {
        return false;
    }
    previous.0 = now;
    true
}

fn valid_quota(previous: &mut (i64, i64), now: i64, (window, issued): (i64, i32)) -> bool {
    if now < previous.0
        || window < previous.1
        || window < 0
        || window > now / 60
        || !(0..=PER_MINUTE).contains(&issued)
    {
        return false;
    }
    previous.1 = window;
    observe_time(previous, now)
}

#[test]
fn quota_rejects_over_limit_and_clock_regression() {
    let mut previous = (120, 2);
    assert!(valid_quota(&mut previous, 120, (2, 64)));
    assert!(!valid_quota(&mut previous, 121, (2, 65)));
    assert!(!valid_quota(&mut previous, 119, (1, 1)));
    assert!(!valid_quota(&mut previous, 120, (3, 1)));
    assert!(!valid_quota(&mut previous, 180, (1, 1)));
    assert!(valid_quota(&mut previous, 180, (3, 1)));
    assert!(!observe_time(&mut previous, 179));
    assert!(observe_time(&mut previous, 181));
}

fn checked_time(previous: &mut (i64, i64)) -> u64 {
    let now = unix_seconds();
    assert!(
        observe_time(previous, i64::try_from(now).expect("bounded clock")),
        "fixture clock moved backwards"
    );
    now
}

async fn read_quota(pool: &PgPool, previous: &mut (i64, i64)) -> (i64, i32) {
    let quota: (i64, i32) =
        sqlx::query_as("SELECT rate_window,issued FROM guest_issue_quota WHERE singleton")
            .fetch_one(pool)
            .await
            .expect("guest issue quota");
    let now = i64::try_from(unix_seconds()).expect("bounded clock");
    assert!(
        valid_quota(previous, now, quota),
        "invalid guest issue quota or clock"
    );
    quota
}

async fn await_issue_capacity(pool: &PgPool, deadline: Instant, previous: &mut (i64, i64)) {
    loop {
        assert!(Instant::now() < deadline, "owner preparation deadline");
        let (window, issued) = read_quota(pool, previous).await;
        if window < previous.0 / 60 || issued < PER_MINUTE {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::test]
#[ignore = "run only through scripts/quote_http_smoke.py --prepare-load"]
async fn owned_quote_load_preparation() {
    prepare(false).await;
}

#[tokio::test]
#[ignore = "run only through scripts/quote_http_smoke.py --run-load"]
async fn owned_quote_load() {
    prepare(true).await;
}

async fn prepare(run_load: bool) {
    let database = owned_database().expect("owned disposable database proof required");
    let pool = PgPool::connect(&database).await.expect("isolated database");
    let empty: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p','v','m','S','f')")
        .fetch_one(&pool).await.expect("empty fixture check");
    assert_eq!(empty, 0, "fresh database required");
    MIGRATOR.run(&pool).await.expect("isolated migrations");
    let seeded_at = unix_seconds();
    fixture::seed_for_load(&pool, seeded_at).await;
    let baseline = effects(&pool).await;
    assert!(
        baseline.0 == 0
            && baseline.1
                == vec![
                    ("SEAT-COVER".into(), 100, 0, 0),
                    ("SEAT-FOAM".into(), 100, 0, 0)
                ]
            && baseline.2 == (0, 0, 0, 0, 0, 0, 0),
        "invalid initial stock or ledger"
    );
    let address = TcpListener::bind("127.0.0.1:0")
        .expect("private port")
        .local_addr()
        .expect("address");
    let bearer = Zeroizing::new(token());
    let child = Command::new(env!("CARGO_BIN_EXE_commerce-api"))
        .env_clear()
        .env("APP_ENV", "test")
        .env("DATABASE_URL", &database)
        .env("PREVIEW_TOKEN", bearer.as_str())
        .env("API_BIND", address.to_string())
        .env("GUEST_ORIGIN", ORIGIN)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("release API");
    let mut server = Server { child, address };
    let listen_deadline = Instant::now() + Duration::from_secs(10);
    while !server.owns_listener() {
        assert!(
            matches!(server.child.try_wait(), Ok(None)),
            "release API exited"
        );
        assert!(
            Instant::now() < listen_deadline,
            "owned listener unavailable"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut owners = Vec::with_capacity(OWNERS);
    let mut tokens = BTreeSet::new();
    let mut proofs = BTreeSet::new();
    let mut baskets = BTreeSet::new();
    let mut windows = BTreeMap::<i64, i32>::new();
    let mut previous_clock = (i64::try_from(seeded_at).expect("bounded clock"), 0);
    for _ in 0..OWNERS {
        await_issue_capacity(&pool, deadline, &mut previous_clock).await;
        let (status, headers, body) = server.request(
            bearer.as_str(),
            "POST",
            "/api/guest/session",
            None,
            None,
            "{}",
        );
        assert_eq!(status, 200, "guest session response");
        let observed_quota = read_quota(&pool, &mut previous_clock).await;
        let (cookie, digest) = cookie_and_digest(&headers);
        assert!(tokens.insert(digest), "guest session duplicated");
        let body = Zeroizing::new(body);
        let session: serde_json::Value = serde_json::from_str(body.as_str()).expect("session JSON");
        assert!(
            session["revision"] == 0 && session["lines"] == serde_json::json!([]),
            "new guest basket invalid"
        );
        let csrf = Zeroizing::new(
            session["csrf_token"]
                .as_str()
                .expect("session proof")
                .to_owned(),
        );
        let _invalid = wrong_csrf(csrf.as_str());
        let proof_hash: [u8; 32] = Sha256::digest(csrf.as_bytes()).into();
        assert!(proofs.insert(proof_hash), "guest proof duplicated");
        let (status, _, body) = server.request(
            bearer.as_str(),
            "PUT",
            "/api/guest/basket",
            Some(cookie.as_str()),
            Some(csrf.as_str()),
            BASKET,
        );
        assert_eq!(status, 200, "basket creation response");
        let body = Zeroizing::new(body);
        let receipt: serde_json::Value = serde_json::from_str(body.as_str()).expect("basket JSON");
        require_basket(&receipt);
        let row: Option<(String, i64, i64, i32, serde_json::Value)> = sqlx::query_as("SELECT b.id,s.created_at,s.expires_at,b.revision,b.lines FROM guest_sessions s JOIN guest_baskets b ON b.id=s.basket_id WHERE s.token_hash=$1 AND s.csrf_token=$2")
            .bind(digest.as_slice()).bind(csrf.as_str()).fetch_optional(&pool).await.expect("exact session custody");
        let (basket, created, expiry, revision, lines) = row.expect("matching persisted session");
        assert!(
            created >= 0
                && expiry > created
                && revision == 1
                && lines == serde_json::json!([{"sku":"SET-FILLED","quantity":3}]),
            "invalid stored owner"
        );
        assert!(baskets.insert(basket.clone()), "guest basket duplicated");
        let independently_counted = windows.entry(observed_quota.0).or_default();
        *independently_counted += 1;
        assert_eq!(
            observed_quota,
            (observed_quota.0, *independently_counted),
            "persisted issuance quota mismatch"
        );
        owners.push(Owner {
            basket,
            cookie,
            csrf,
        });
    }
    assert!(windows.len() >= 2, "guest quota rollover not observed");
    assert!(
        checked_time(&mut previous_clock) <= seeded_at + 180,
        "fixture setup exceeded duration"
    );
    for owner in &owners {
        assert!(Instant::now() < deadline, "owner preparation deadline");
        let (status, _, body) = server.request(
            bearer.as_str(),
            "GET",
            "/api/guest/basket",
            Some(owner.cookie.as_str()),
            None,
            "",
        );
        assert_eq!(status, 200, "prepared owner read");
        let body = Zeroizing::new(body);
        let basket: serde_json::Value = serde_json::from_str(body.as_str()).expect("basket JSON");
        assert!(
            basket
                == serde_json::json!({"revision":1,"lines":[{"sku":"SET-FILLED","quantity":3}],"csrf_token":owner.csrf.as_str()}),
            "prepared owner binding invalid"
        );
    }
    let (denied, _, _) = server.request(
        bearer.as_str(),
        "PUT",
        "/api/guest/basket",
        Some(owners[0].cookie.as_str()),
        Some(owners[1].csrf.as_str()),
        BASKET,
    );
    assert_eq!(denied, 403, "foreign session proof denied");
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM guest_sessions WHERE expires_at >= $1")
            .bind(i64::try_from(checked_time(&mut previous_clock) + 660).expect("bounded clock"))
            .fetch_one(&pool)
            .await
            .expect("remaining session lifetime");
    let counts: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM guest_sessions),(SELECT count(*) FROM guest_baskets),(SELECT count(*) FROM guest_basket_effects)")
        .fetch_one(&pool).await.expect("prepared owner counts");
    assert!(
        owners.len() == OWNERS && remaining == 75 && counts == (75, 75, 75),
        "incomplete owner set"
    );
    assert_eq!(
        effects(&pool).await,
        baseline,
        "preparation changed stock or commerce ledgers"
    );
    if run_load {
        driver::run(&mut server, bearer.as_str(), &owners, &pool, baseline).await;
    }
}
