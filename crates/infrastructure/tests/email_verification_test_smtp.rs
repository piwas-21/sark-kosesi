#![cfg(feature = "test-verification-smtp")]
//! Real loopback SMTP with synthetic recipients and isolated PostgreSQL.
use commerce_application::request_context::{Clock, ContextError};
use commerce_infrastructure::{
    MIGRATOR,
    customer_credential_admission::{AdmissionKey, AdmissionLimits},
    email_verification::{
        VerificationToken,
        delivery::{
            VerificationDeliveryKey, issue_for_delivery,
            opening::OpenOutcome,
            outcome::{DeliveryState, TransportOutcome, inspect},
            register_key,
            test_smtp::{LocalTestEndpoint, deliver},
        },
        verified_customer_accounts,
    },
};
use sqlx::PgPool;
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::oneshot,
};
use zeroize::Zeroizing;

const ADDRESS: &str = "verify@example.test";
struct Time(AtomicU64);
impl Clock for Time {
    fn unix_seconds(&self) -> Result<u64, ContextError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
fn admission() -> AdmissionKey {
    AdmissionKey::new(1, [27; 32]).expect("admission")
}
fn limits() -> AdmissionLimits {
    AdmissionLimits::new(1000, 100).expect("limits")
}
async fn fixture(pool: &PgPool) -> (Time, VerificationDeliveryKey, [u8; 32]) {
    let time = Time(AtomicU64::new(1000));
    let key = VerificationDeliveryKey::capture("smtp-key", [41; 32]).expect("key");
    register_key(pool, &time, &key).await.expect("register");
    let receipt = issue_for_delivery(pool, ADDRESS, &time, &admission(), limits(), &key)
        .await
        .expect("issue");
    (time, key, receipt.verifier)
}

#[derive(Clone, Copy)]
enum Reply {
    Accept,
    AcceptThenStall,
    Reject,
    TemporaryReject,
    Disconnect,
}

struct Rendezvous {
    ready: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

// One connection only. No forwarding, filesystem storage, DNS or external mail.
async fn peer(
    listener: &TcpListener,
    pool: &PgPool,
    verifier: &[u8; 32],
    reply: Reply,
    rendezvous: Option<Rendezvous>,
) -> Zeroizing<String> {
    let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("connection deadline")
        .expect("accept local connection");
    let mut stream = BufReader::new(stream);
    stream
        .get_mut()
        .write_all(b"220 fixture\r\n")
        .await
        .expect("greeting");
    let mut line = String::new();
    for (prefix, response) in [
        ("EHLO ", "250 fixture\r\n"),
        ("MAIL FROM:<sender@example.test>", "250 ok\r\n"),
        ("RCPT TO:<verify@example.test>", "250 ok\r\n"),
        ("DATA", "354 send data\r\n"),
    ] {
        line.clear();
        stream.read_line(&mut line).await.expect("command");
        assert!(line.starts_with(prefix));
        stream
            .get_mut()
            .write_all(response.as_bytes())
            .await
            .expect("response");
    }
    let mut message = Zeroizing::new(String::new());
    loop {
        line.clear();
        stream.read_line(&mut line).await.expect("data line");
        assert!(!line.is_empty());
        if line == ".\r\n" {
            break;
        }
        message.push_str(&line);
        assert!(message.len() < 1024);
    }
    // The peer receives DATA only after opening commits; a new DB writer can lock
    // the challenge while this SMTP connection is active.
    let mut probe = pool.begin().await.expect("begin lock probe");
    sqlx::query(
        "SELECT verifier FROM email_verification_challenges WHERE verifier=$1 FOR UPDATE NOWAIT",
    )
    .bind(verifier.as_slice())
    .fetch_one(&mut *probe)
    .await
    .expect("no database lock across SMTP");
    probe.commit().await.expect("release probe lock");
    if let Some(rendezvous) = rendezvous {
        rendezvous.ready.send(()).expect("signal lock probe");
        tokio::time::timeout(Duration::from_secs(5), rendezvous.release)
            .await
            .expect("release deadline")
            .expect("release sender");
    }
    match reply {
        Reply::Disconnect => return message,
        Reply::Accept | Reply::AcceptThenStall => stream
            .get_mut()
            .write_all(b"250 queued\r\n")
            .await
            .expect("accepted"),
        Reply::Reject => stream
            .get_mut()
            .write_all(b"550 rejected\r\n")
            .await
            .expect("rejected"),
        Reply::TemporaryReject => stream
            .get_mut()
            .write_all(b"450 rejected\r\n")
            .await
            .expect("temporarily rejected"),
    }
    if matches!(reply, Reply::AcceptThenStall) {
        tokio::time::sleep(Duration::from_secs(3)).await;
        return message;
    }
    line.clear();
    if stream.read_line(&mut line).await.unwrap_or(0) > 0 && line.starts_with("QUIT") {
        stream
            .get_mut()
            .write_all(b"221 bye\r\n")
            .await
            .expect("quit");
    }
    message
}

fn proof(message: &str) -> VerificationToken {
    let encoded = message
        .lines()
        .find_map(|line| line.strip_prefix("proof="))
        .expect("proof");
    assert_eq!(encoded.len(), 64);
    let mut bytes = [0; 32];
    for (i, chunk) in encoded.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let hex = std::str::from_utf8(chunk).expect("hex");
        bytes[i] = u8::from_str_radix(hex, 16).expect("hex byte");
    }
    VerificationToken::from_bytes(bytes)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn delivered_message_redeems_and_concurrent_openers_send_once(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listen");
    let endpoint =
        LocalTestEndpoint::new(true, listener.local_addr().expect("address")).expect("local");
    let (ready_tx, ready_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let competitor = async {
        tokio::time::timeout(Duration::from_secs(5), ready_rx)
            .await
            .expect("lock probe deadline")
            .expect("lock probe sender");
        // The first sender is waiting for the SMTP response after the peer's
        // lock probe. A duplicate connection at any point is a test failure.
        let result = tokio::select! {
            biased;
            connection = listener.accept() => panic!("duplicate SMTP connection: {connection:?}"),
            result = tokio::time::timeout(
                Duration::from_secs(5),
                deliver(&pool, &verifier, &time, &key, &endpoint),
            ) => result.expect("competitor deadline"),
        };
        assert_eq!(result, Ok(OpenOutcome::AlreadyStarted));
        assert_eq!(inspect(&pool, &verifier).await, Ok(DeliveryState::Unknown));
        let counts: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM email_verification_delivery_starts WHERE verifier=$1), (SELECT count(*) FROM email_verification_delivery_outcomes WHERE verifier=$1)",
        )
        .bind(verifier.as_slice())
        .fetch_one(&pool)
        .await
        .expect("delivery row counts");
        assert_eq!(counts, (1, 0));
        tokio::select! {
            biased;
            connection = listener.accept() => panic!("duplicate SMTP connection: {connection:?}"),
            () = std::future::ready(()) => {},
        }
        release_tx.send(()).expect("release peer");
    };
    let (message, result, ()) = tokio::join!(
        peer(
            &listener,
            &pool,
            &verifier,
            Reply::Accept,
            Some(Rendezvous {
                ready: ready_tx,
                release: release_rx,
            })
        ),
        deliver(&pool, &verifier, &time, &key, &endpoint),
        competitor,
    );
    assert_eq!(result, Ok(OpenOutcome::Opened));
    assert!(message.starts_with("From: sender@example.test\r\nTo: verify@example.test\r\n"));
    assert_eq!(
        inspect(&pool, &verifier).await,
        Ok(DeliveryState::Recorded(TransportOutcome::Accepted))
    );
    let stored: String = sqlx::query_scalar(
        "SELECT row_to_json(o)::text FROM email_verification_delivery_outcomes o WHERE verifier=$1",
    )
    .bind(verifier.as_slice())
    .fetch_one(&pool)
    .await
    .expect("outcome row");
    let encoded = message
        .lines()
        .find_map(|line| line.strip_prefix("proof="))
        .expect("proof");
    assert!(!stored.contains(ADDRESS));
    assert!(!stored.contains(encoded));
    let token = proof(&message);
    assert!(
        verified_customer_accounts::establish(
            &pool,
            ADDRESS,
            &token,
            &time,
            &admission(),
            limits()
        )
        .await
        .is_ok()
    );
    assert!(
        verified_customer_accounts::establish(
            &pool,
            ADDRESS,
            &token,
            &time,
            &admission(),
            limits()
        )
        .await
        .is_err()
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn explicit_rejection_and_missing_reply_never_resend(pool: PgPool) {
    for (reply, expected) in [
        (Reply::Reject, TransportOutcome::ExplicitRejection),
        (Reply::TemporaryReject, TransportOutcome::ExplicitRejection),
        (Reply::Disconnect, TransportOutcome::Uncertain),
        (Reply::AcceptThenStall, TransportOutcome::Uncertain),
    ] {
        let (time, key, verifier) = fixture(&pool).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listen");
        let endpoint =
            LocalTestEndpoint::new(true, listener.local_addr().expect("address")).expect("local");
        let (_message, result) = tokio::join!(
            peer(&listener, &pool, &verifier, reply, None),
            deliver(&pool, &verifier, &time, &key, &endpoint)
        );
        assert_eq!(result, Ok(OpenOutcome::Opened));
        assert_eq!(
            inspect(&pool, &verifier).await,
            Ok(DeliveryState::Recorded(expected))
        );
        assert_eq!(
            deliver(&pool, &verifier, &time, &key, &endpoint).await,
            Ok(OpenOutcome::AlreadyStarted)
        );
    }
}

#[tokio::test]
async fn endpoint_requires_explicit_opt_in_and_exact_loopback() {
    for address in ["192.0.2.1:25", "127.0.0.2:25", "0.0.0.0:25", "127.0.0.1:0"] {
        assert!(
            LocalTestEndpoint::new(true, address.parse::<SocketAddr>().expect("address")).is_err()
        );
    }
    assert!(LocalTestEndpoint::new(false, "127.0.0.1:25".parse().expect("address")).is_err());
    assert!(LocalTestEndpoint::new(true, "[::1]:25".parse().expect("address")).is_ok());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn accepted_message_with_failed_record_stays_unknown(pool: PgPool) {
    use sqlx::Executor;
    let (time, key, verifier) = fixture(&pool).await;
    pool.execute("CREATE FUNCTION smtp_test_suppress_outcome() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$; CREATE TRIGGER smtp_test_suppress BEFORE INSERT ON email_verification_delivery_outcomes FOR EACH ROW EXECUTE FUNCTION smtp_test_suppress_outcome()").await.expect("suppress outcome");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listen");
    let endpoint =
        LocalTestEndpoint::new(true, listener.local_addr().expect("address")).expect("local");
    let (_message, result) = tokio::join!(
        peer(&listener, &pool, &verifier, Reply::Accept, None),
        deliver(&pool, &verifier, &time, &key, &endpoint)
    );
    assert!(result.is_err());
    assert_eq!(inspect(&pool, &verifier).await, Ok(DeliveryState::Unknown));
    assert_eq!(
        deliver(&pool, &verifier, &time, &key, &endpoint).await,
        Ok(OpenOutcome::AlreadyStarted)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn different_mailbox_is_rejected_before_start(pool: PgPool) {
    let time = Time(AtomicU64::new(1000));
    let key = VerificationDeliveryKey::capture("smtp-key", [41; 32]).expect("key");
    register_key(&pool, &time, &key).await.expect("register");
    let receipt = issue_for_delivery(
        &pool,
        "other@example.test",
        &time,
        &admission(),
        limits(),
        &key,
    )
    .await
    .expect("issue");
    let endpoint =
        LocalTestEndpoint::new(true, "127.0.0.1:25".parse().expect("address")).expect("local");
    assert!(
        deliver(&pool, &receipt.verifier, &time, &key, &endpoint)
            .await
            .is_err()
    );
    assert_eq!(
        inspect(&pool, &receipt.verifier).await,
        Ok(DeliveryState::NotStarted)
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unavailable_database_denies_before_connecting_to_smtp(pool: PgPool) {
    let (time, key, verifier) = fixture(&pool).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listen");
    let endpoint =
        LocalTestEndpoint::new(true, listener.local_addr().expect("address")).expect("local");
    pool.close().await;
    assert!(
        deliver(&pool, &verifier, &time, &key, &endpoint)
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}
