//! Real local SMTP peer and guest proof start/outcome custody, with no external mail.
use super::*;
use commerce_infrastructure::guest_order_links::delivery::test_smtp::{LocalTestEndpoint, deliver};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};
use zeroize::Zeroizing;

async fn establish_listener() -> (TcpListener, LocalTestEndpoint) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("local listener");
    let endpoint =
        LocalTestEndpoint::new(true, listener.local_addr().expect("address")).expect("local sink");
    (listener, endpoint)
}

async fn peer(
    listener: &TcpListener,
    pool: &PgPool,
    verifier: &[u8; 32],
    final_reply: Option<&str>,
) {
    let (connection, _) = listener.accept().await.expect("one local delivery");
    let mut stream = BufReader::new(connection);
    stream
        .get_mut()
        .write_all(b"220 fixture\r\n")
        .await
        .expect("greeting");
    let mut line = Zeroizing::new(String::new());
    for (expected, reply) in [
        ("EHLO example.test\r\n", "250 fixture\r\n"),
        ("MAIL FROM:<sender@example.test>\r\n", "250 fixture\r\n"),
        ("RCPT TO:<buyer@example.test>\r\n", "250 fixture\r\n"),
        ("DATA\r\n", "354 send data\r\n"),
    ] {
        line.clear();
        stream.read_line(&mut line).await.expect("SMTP command");
        assert!(line.as_str() == expected, "unexpected test SMTP command");
        stream
            .get_mut()
            .write_all(reply.as_bytes())
            .await
            .expect("SMTP response");
    }
    for expected in [
        "From: sender@example.test\r\n",
        "To: buyer@example.test\r\n",
        "Subject: Synthetic guest order link\r\n",
        "Content-Type: text/plain; charset=us-ascii\r\n",
        "\r\n",
    ] {
        line.clear();
        stream.read_line(&mut line).await.expect("SMTP header");
        assert!(line.as_str() == expected, "unexpected test SMTP header");
    }
    line.clear();
    stream.read_line(&mut line).await.expect("SMTP proof");
    let proof = line
        .strip_prefix("proof=")
        .and_then(|line| line.strip_suffix("\r\n"))
        .expect("fixed proof line");
    assert_eq!(proof.len(), 64);
    let mut bytes = Zeroizing::new([0_u8; 32]);
    for (byte, chunk) in bytes.iter_mut().zip(proof.as_bytes().as_chunks::<2>().0) {
        *byte = u8::from_str_radix(std::str::from_utf8(chunk).expect("hex"), 16).expect("hex byte");
    }
    let delivered_verifier: [u8; 32] = Sha256::new()
        .chain_update(b"piwas:guest-order-link:verifier:v1\0")
        .chain_update(bytes.as_slice())
        .finalize()
        .into();
    assert!(
        &delivered_verifier == verifier,
        "delivery must carry the original proof"
    );
    line.clear();
    stream.read_line(&mut line).await.expect("SMTP end marker");
    assert!(
        line.as_str() == ".\r\n",
        "unexpected SMTP data after the one proof"
    );

    // The peer owns the final reply, while another transaction can see and lock
    // the proof. A duplicate opener must still see the committed start.
    assert_eq!(
        outcome::inspect(pool, verifier).await,
        Ok(DeliveryState::Unknown)
    );
    let mut check = pool.begin().await.expect("proof lock probe");
    sqlx::query("SELECT verifier FROM guest_order_link_proofs WHERE verifier=$1 FOR UPDATE NOWAIT")
        .bind(verifier.as_slice())
        .fetch_one(&mut *check)
        .await
        .expect("no held proof lock over SMTP");
    check.commit().await.expect("release proof probe");
    if let Some(reply) = final_reply {
        stream
            .get_mut()
            .write_all(reply.as_bytes())
            .await
            .expect("final response");
        line.clear();
        if stream.read_line(&mut line).await.expect("optional quit") != 0 {
            assert!(line.as_str() == "QUIT\r\n", "unexpected final command");
            stream
                .get_mut()
                .write_all(b"221 bye\r\n")
                .await
                .expect("quit response");
        }
    }
}

async fn exercise(pool: PgPool, final_reply: Option<&str>, expected: DeliveryState) {
    let (clock, key, verifier, _) = sample(&pool, "smtp").await;
    let (listener, endpoint) = establish_listener().await;
    if expected == DeliveryState::Unknown {
        pool.execute("CREATE FUNCTION suppress_test_link_smtp_outcome() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$; CREATE TRIGGER z_suppress_test_link_smtp_outcome BEFORE INSERT ON guest_link_delivery_outcomes FOR EACH ROW EXECUTE FUNCTION suppress_test_link_smtp_outcome()")
            .await.expect("simulate outcome persistence failure");
    }
    let ((), result) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            peer(&listener, &pool, &verifier, final_reply),
            deliver(&pool, &verifier, clock.as_ref(), &key, &endpoint)
        )
    })
    .await
    .expect("bounded SMTP and custody work");
    assert_eq!(
        result,
        if expected == DeliveryState::Unknown {
            Err(LinkError::Unavailable)
        } else {
            Ok(OpenOutcome::Opened)
        }
    );
    assert_eq!(outcome::inspect(&pool, &verifier).await, Ok(expected));
    assert_eq!(
        deliver(&pool, &verifier, clock.as_ref(), &key, &endpoint).await,
        Ok(OpenOutcome::AlreadyStarted)
    );
    tokio::select! {
        biased;
        connection = listener.accept() => { drop(connection); panic!("duplicate SMTP delivery"); }
        () = std::future::ready(()) => {},
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn local_guest_link_records_acceptance_once(pool: PgPool) {
    exercise(
        pool,
        Some("250 queued\r\n"),
        DeliveryState::Recorded(TransportOutcome::Accepted),
    )
    .await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn local_guest_link_records_rejection_once(pool: PgPool) {
    exercise(
        pool,
        Some("550 rejected\r\n"),
        DeliveryState::Recorded(TransportOutcome::ExplicitRejection),
    )
    .await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn local_guest_link_lost_reply_is_uncertain(pool: PgPool) {
    exercise(
        pool,
        None,
        DeliveryState::Recorded(TransportOutcome::Uncertain),
    )
    .await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn local_guest_link_lost_outcome_stays_unknown(pool: PgPool) {
    exercise(pool, Some("250 queued\r\n"), DeliveryState::Unknown).await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn local_guest_link_only_starts_for_the_reserved_buyer(pool: PgPool) {
    let (clock, key, verifier, _) =
        sample_with_buyer(&pool, "foreign-smtp", "other@example.test").await;
    let (listener, endpoint) = establish_listener().await;
    assert_eq!(
        deliver(&pool, &verifier, clock.as_ref(), &key, &endpoint).await,
        Err(LinkError::Denied)
    );
    assert_eq!(
        outcome::inspect(&pool, &verifier).await,
        Ok(DeliveryState::NotStarted)
    );
    tokio::select! {
        biased;
        connection = listener.accept() => { drop(connection); panic!("delivery for a different buyer"); }
        () = std::future::ready(()) => {},
    }
}
