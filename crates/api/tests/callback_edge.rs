#[path = "support/callback_edge_binary.rs"]
mod binary;
#[path = "support/callback_edge_database.rs"]
mod database;
#[path = "support/callback_edge_support.rs"]
mod support;
use axum::{body::Body, extract::ConnectInfo, http::Request};
use commerce_api::callback_edge::{Config, router};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use support::*;
use tokio::{io::AsyncWriteExt, net::TcpStream, sync::Notify};
use tower::ServiceExt;

#[test]
fn edge_is_explicitly_disabled_and_enabled_configuration_is_closed() {
    assert!(
        Config::parse(&HashMap::new())
            .expect("absent flag disables edge")
            .is_none()
    );
    let mut disabled = HashMap::from([("CALLBACK_EDGE_ENABLED".into(), "false".into())]);
    assert!(
        Config::parse(&disabled)
            .expect("explicitly disabled")
            .is_none()
    );
    disabled.insert("APP_ENV".into(), "production".into());
    assert!(
        Config::parse(&disabled).is_err(),
        "production rejected even when disabled"
    );
    let valid = values("127.0.0.1:45671".parse().expect("loopback upstream"));
    let config = configured(&valid);
    assert_eq!(
        config.bind(),
        "127.0.0.1:0".parse::<SocketAddr>().expect("literal bind")
    );
    assert!(config.revision().is_none());
    for key in [
        "CALLBACK_EDGE_BIND",
        "CALLBACK_EDGE_PROXY_IPS",
        "API_URL",
        "PREVIEW_TOKEN",
    ] {
        let mut missing = valid.clone();
        missing.remove(key);
        assert!(
            Config::parse(&missing).is_err(),
            "enabled edge requires {key}"
        );
    }
    for (key, value) in [
        ("APP_ENV", "production"),
        ("APP_ENV", "unknown"),
        ("CALLBACK_EDGE_ENABLED", "TRUE"),
        ("CALLBACK_EDGE_ENABLED", ""),
        ("CALLBACK_EDGE_BIND", "invalid"),
        ("CALLBACK_EDGE_BIND", "0.0.0.0:45672"),
        ("CALLBACK_EDGE_PROXY_IPS", ""),
        ("CALLBACK_EDGE_PROXY_IPS", "proxy.internal"),
        ("CALLBACK_EDGE_PROXY_IPS", "127.0.0.0/8"),
        ("CALLBACK_EDGE_PROXY_IPS", "127.0.0.1:1234"),
        ("CALLBACK_EDGE_PROXY_IPS", "127.0.0.1,127.0.0.1"),
        (
            "CALLBACK_EDGE_PROXY_IPS",
            "127.0.0.1,127.0.0.2,127.0.0.3,127.0.0.4,127.0.0.5",
        ),
        ("CALLBACK_EDGE_PROXY_IPS", "127.0.0.1,"),
        ("CALLBACK_EDGE_PROXY_IPS", "0.0.0.0"),
        ("CALLBACK_EDGE_PROXY_IPS", "8.8.8.8"),
        ("CALLBACK_EDGE_PROXY_IPS", "169.254.1.2"),
        ("CALLBACK_EDGE_PROXY_IPS", "fe80::1"),
        ("CALLBACK_EDGE_PROXY_IPS", "ff02::1"),
        ("API_URL", "https://127.0.0.1:45671"),
        ("API_URL", "http://localhost:45671"),
        ("API_URL", "http://8.8.8.8:45671"),
        ("API_URL", "http://127.0.0.1:0"),
        ("API_URL", "http://127.0.0.1:45671/private"),
        ("API_URL", "http://127.0.0.1:45671?token=private"),
        ("API_URL", "http://user:private@127.0.0.1:45671"),
        ("API_URL", "http://127.0.0.1:45671#private"),
        ("PREVIEW_TOKEN", "short"),
        ("PREVIEW_TOKEN", "fixture_invalid_token_with_private!"),
        ("PREVIEW_REVISION", "short"),
    ] {
        let mut invalid = valid.clone();
        invalid.insert(key.into(), value.into());
        assert!(
            Config::parse(&invalid).is_err(),
            "invalid {key} must be rejected"
        );
    }
    let mut four = valid.clone();
    four.insert(
        "CALLBACK_EDGE_PROXY_IPS".into(),
        "127.0.0.1,10.1.2.3,172.16.0.2,fd12::7".into(),
    );
    assert!(
        Config::parse(&four).is_ok(),
        "one through four exact distinct peers are allowed"
    );
    let mut ipv6 = valid.clone();
    ipv6.insert("CALLBACK_EDGE_BIND".into(), "[::1]:0".into());
    ipv6.insert("API_URL".into(), "http://[::1]:45671".into());
    ipv6.insert("CALLBACK_EDGE_PROXY_IPS".into(), "::1".into());
    assert!(
        Config::parse(&ipv6).is_ok(),
        "literal loopback IPv6 is supported in test mode"
    );
}

#[test]
fn staging_requires_private_exact_peers_the_dedicated_port_and_source_revision() {
    let mut stage = values("127.0.0.1:45671".parse().expect("upstream"));
    stage.insert("APP_ENV".into(), "staging".into());
    stage.insert("CALLBACK_EDGE_BIND".into(), "0.0.0.0:45672".into());
    stage.insert(
        "CALLBACK_EDGE_PROXY_IPS".into(),
        "10.1.2.3,172.16.0.7,192.168.1.5,fd12::8".into(),
    );
    let revision = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(revision.len(), 40);
    assert!(
        Config::parse(&stage).is_err(),
        "staging requires a source revision"
    );
    stage.insert("PREVIEW_REVISION".into(), revision.into());
    let config = configured(&stage);
    assert_eq!(config.bind().port(), 45672);
    assert_eq!(config.revision(), Some(revision));
    for (key, value) in [
        ("CALLBACK_EDGE_BIND", "0.0.0.0:45671"),
        ("CALLBACK_EDGE_BIND", "0.0.0.0:0"),
        ("CALLBACK_EDGE_PROXY_IPS", "127.0.0.1"),
        ("CALLBACK_EDGE_PROXY_IPS", "::1"),
        ("CALLBACK_EDGE_PROXY_IPS", "192.0.2.1"),
        ("CALLBACK_EDGE_PROXY_IPS", "10.0.0.0/8"),
        (
            "PREVIEW_REVISION",
            "0123456789ABCDEF0123456789ABCDEF01234567",
        ),
        (
            "PREVIEW_REVISION",
            "g123456789abcdef0123456789abcdef01234567",
        ),
    ] {
        let mut invalid = stage.clone();
        invalid.insert(key.into(), value.into());
        assert!(
            Config::parse(&invalid).is_err(),
            "staging rejects invalid {key}"
        );
    }
}

#[tokio::test]
async fn actual_socket_peer_is_required_before_body_read_and_forwarded_headers_never_grant_trust() {
    let upstream = Stub::start(Behavior::Reply(empty_200())).await;
    let mut denied = values(upstream.address);
    denied.insert("CALLBACK_EDGE_PROXY_IPS".into(), "127.0.0.2".into());
    let edge = Server::start(router(configured(&denied))).await;
    let mut stream = TcpStream::connect(edge.address)
        .await
        .expect("actual untrusted peer");
    stream
        .write_all(&head(
            "POST",
            PATH,
            "attacker.invalid",
            Some("application/x-www-form-urlencoded"),
            &[
                ("X-Forwarded-For", "127.0.0.2"),
                ("X-Real-IP", "127.0.0.2"),
                ("Forwarded", "for=127.0.0.2"),
            ],
            100,
        ))
        .await
        .expect("untrusted headers with no body");
    let response = tokio::time::timeout(Duration::from_millis(500), read_response(&mut stream))
        .await
        .expect("peer rejection precedes one-second body wait");
    assert_eq!(response.status, 403);
    safe_failure(&response);
    assert!(upstream.records().is_empty());
    assert_eq!(
        upstream.connections.load(Ordering::SeqCst),
        0,
        "rejected input cannot even open an upstream connection"
    );
    let missing = router(configured(&values(upstream.address)))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(PATH)
                .body(Body::from("id=tr_MISSING_PEER"))
                .expect("request without connection metadata"),
        )
        .await
        .expect("missing peer response");
    assert_eq!(missing.status(), 403);
    let mut wrong = Request::builder()
        .method("POST")
        .uri(PATH)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("id=tr_WRONG_EXTENSION"))
        .expect("request");
    wrong.extensions_mut().insert(ConnectInfo(
        "127.0.0.2:9"
            .parse::<SocketAddr>()
            .expect("denied connection metadata"),
    ));
    assert_eq!(
        router(configured(&values(upstream.address)))
            .oneshot(wrong)
            .await
            .expect("wrong metadata response")
            .status(),
        403
    );
    edge.stop().await;
}

#[tokio::test]
async fn forwarding_uses_only_server_credentials_fixed_path_and_canonical_media_type() {
    let upstream = Stub::start(Behavior::Reply(empty_200())).await;
    let edge = Server::start(router(configured(&values(upstream.address)))).await;
    let body = b"%69d=tr_EDGE%2Bopaque";
    let mut bytes = head(
        "POST",
        &format!("{PATH}?private=caller-query-marker"),
        "caller-host-marker.invalid",
        Some("APPLICATION/X-WWW-FORM-URLENCODED; CHARSET=\"UTF-8\""),
        &[
            ("Authorization", "Bearer caller-auth-marker"),
            ("Cookie", "private=caller-cookie-marker"),
            ("Origin", "https://caller-origin-marker.invalid"),
            ("X-Forwarded-For", "caller-xff-marker"),
            ("X-Forwarded-Host", "caller-forward-host-marker"),
            ("X-Real-IP", "caller-ip-marker"),
            ("Forwarded", "for=caller-forwarded-marker"),
            ("X-Request-ID", "caller-request-marker"),
            ("X-Private-Input", "caller-custom-marker"),
        ],
        body.len(),
    );
    bytes.extend_from_slice(body);
    let response = exchange(edge.address, bytes).await;
    assert_eq!(response.status, 200);
    assert!(response.body.is_empty());
    upstream.wait_for(1).await;
    let records = upstream.records();
    assert_eq!(records.len(), 1);
    let request = &records[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, PATH);
    assert_eq!(request.body, body);
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert_eq!(
        request.header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        request.header("host"),
        Some(upstream.address.to_string().as_str())
    );
    for key in [
        "cookie",
        "origin",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-real-ip",
        "forwarded",
        "x-request-id",
        "x-private-input",
        "referer",
    ] {
        assert!(
            request.header(key).is_none(),
            "caller header {key} cannot be copied"
        );
    }
    for (key, _) in &request.headers {
        assert!(
            [
                "authorization",
                "content-type",
                "host",
                "content-length",
                "accept"
            ]
            .contains(&key.as_str()),
            "unexpected outbound header"
        );
    }
    edge.stop().await;
}

#[tokio::test]
async fn only_finished_empty_upstream_200_can_acknowledge_a_callback() {
    let release = Arc::new(Notify::new());
    let sent = Arc::new(Notify::new());
    let upstream = Stub::start(Behavior::HeadersThenHold {
        release: release.clone(),
        sent: sent.clone(),
    })
    .await;
    let edge = Server::start(router(configured(&values(upstream.address)))).await;
    let address = edge.address;
    let mut task = tokio::spawn(async move { post(address, b"id=tr_FINISHED_RESPONSE").await });
    upstream.wait_for(1).await;
    tokio::time::timeout(Duration::from_secs(1), sent.notified())
        .await
        .expect("upstream 200 headers actually written");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut task)
            .await
            .is_err(),
        "response headers alone do not prove an empty completed upstream body"
    );
    release.notify_waiters();
    let response = task.await.expect("acknowledgement task");
    assert_eq!(response.status, 200);
    assert!(response.body.is_empty());
    edge.stop().await;
    for raw in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nX-Upstream-Private: upstream-header-marker\r\nConnection: close\r\n\r\nx".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\n".to_vec(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n1\r\nx\r\n0\r\n\r\n".to_vec(),
        b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec(),
        b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic private-realm\r\nSet-Cookie: private=upstream-cookie-marker\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 23\r\nConnection: close\r\n\r\nupstream-private-marker".to_vec(),
    ] {
        let upstream=Stub::start(Behavior::Reply(raw)).await;
        let edge=Server::start(router(configured(&values(upstream.address)))).await;
        let response=post(edge.address,b"id=tr_BAD_UPSTREAM").await;
        assert_eq!(response.status,503); safe_failure(&response);
        assert_eq!(response.body,b"{\"code\":\"callback_edge_unavailable\"}");
        assert_eq!(upstream.records().len(),1);
        edge.stop().await;
    }
}

#[tokio::test]
async fn redirects_post_replays_and_slow_upstream_results_cannot_mint_success() {
    let target = Stub::start(Behavior::Reply(empty_200())).await;
    let redirect = format!(
        "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{}/private-redirect-target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        target.address
    );
    let upstream = Stub::start(Behavior::Reply(redirect.into_bytes())).await;
    let edge = Server::start(router(configured(&values(upstream.address)))).await;
    let response = post(edge.address, b"id=tr_REDIRECT").await;
    assert_eq!(response.status, 503);
    safe_failure(&response);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(upstream.records().len(), 1);
    assert!(
        target.records().is_empty(),
        "redirect target must never be contacted"
    );
    edge.stop().await;
    let upstream = Stub::start(Behavior::Disconnect).await;
    let edge = Server::start(router(configured(&values(upstream.address)))).await;
    let response = post(edge.address, b"id=tr_UNKNOWN_POST_RESULT").await;
    assert_eq!(response.status, 503);
    safe_failure(&response);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        upstream.records().len(),
        1,
        "an unknown POST result cannot be submitted again automatically"
    );
    assert_eq!(upstream.connections.load(Ordering::SeqCst), 1);
    edge.stop().await;
    let upstream = Stub::start(Behavior::Delay(Duration::from_secs(5))).await;
    let edge = Server::start(router(configured(&values(upstream.address)))).await;
    let began = Instant::now();
    let response = post(edge.address, b"id=tr_SLOW_UPSTREAM").await;
    assert_eq!(response.status, 503);
    safe_failure(&response);
    assert!(
        began.elapsed() < Duration::from_millis(2500),
        "the 1500ms upstream exchange budget expires before the 3s total budget"
    );
    edge.stop().await;
}

#[tokio::test]
async fn sixteen_inflight_requests_are_global_across_caller_host_variants() {
    let release = Arc::new(Notify::new());
    let upstream = Stub::start(Behavior::Hold(release.clone())).await;
    let edge = Server::start(router(configured(&values(upstream.address)))).await;
    let mut tasks = Vec::new();
    for n in 0..16 {
        let address = edge.address;
        tasks.push(tokio::spawn(async move {
            let body = format!("id=tr_GLOBAL_{n}");
            let mut bytes = head(
                "POST",
                PATH,
                &format!("caller-{n}.invalid"),
                Some("application/x-www-form-urlencoded"),
                &[],
                body.len(),
            );
            bytes.extend_from_slice(body.as_bytes());
            exchange(address, bytes).await
        }));
    }
    upstream.wait_for(16).await;
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        post(edge.address, b"id=tr_SEVENTEENTH_HOST"),
    )
    .await
    .expect("literal global capacity rejects immediately");
    assert_eq!(response.status, 503);
    assert_eq!(upstream.records().len(), 16);
    release.notify_waiters();
    for task in tasks {
        let response = task.await.expect("held request");
        assert_eq!(response.status, 200);
        assert!(response.body.is_empty());
    }
    let address = edge.address;
    let recovered = tokio::spawn(async move { post(address, b"id=tr_SLOT_RECOVERED").await });
    upstream.wait_for(17).await;
    release.notify_waiters();
    assert_eq!(recovered.await.expect("capacity recovered").status, 200);
    edge.stop().await;
}

#[tokio::test]
async fn form_headers_and_raw_streaming_limits_reject_before_any_upstream_effect() {
    let upstream = Stub::start(Behavior::Reply(empty_200())).await;
    let edge = Server::start(router(configured(&values(upstream.address)))).await;
    for (body, expected) in [
        (b"id=tr_one&id=tr_two".as_slice(), 400),
        (b"id=tr_one&%69d=tr_two", 400),
        (b"id=tr_one&status=paid", 400),
        (b"id=tr_%ff", 400),
        (b"id=tr_%GG", 400),
        (b"id=tr_%00", 400),
    ] {
        let response = post(edge.address, body).await;
        assert_eq!(response.status, expected);
        safe_failure(&response);
    }
    for (length, expected) in [(1024, 400), (1025, 413)] {
        let body = format!("id=tr_{}", "x".repeat(length - 6));
        assert_eq!(body.len(), length);
        let response = post(edge.address, body.as_bytes()).await;
        assert_eq!(response.status, expected);
    }
    for (media, extras) in [
        (None, vec![]),
        (Some("application/json"), vec![]),
        (
            Some("application/x-www-form-urlencoded"),
            vec![("Content-Encoding", "gzip")],
        ),
        (
            Some("application/x-www-form-urlencoded"),
            vec![("Content-Type", "application/x-www-form-urlencoded")],
        ),
    ] {
        let mut bytes = head("POST", PATH, "edge.invalid", media, &extras, 10);
        bytes.extend_from_slice(b"id=tr_TEST");
        let response = exchange(edge.address, bytes).await;
        assert_eq!(response.status, 415);
        safe_failure(&response);
    }
    assert!(upstream.records().is_empty());
    assert_eq!(
        upstream.connections.load(Ordering::SeqCst),
        0,
        "rejected input cannot even open an upstream connection"
    );
    let base = "application/x-www-form-urlencoded";
    let parameter = ";charset=utf-8";
    let media = format!(
        "{base}{}{parameter}",
        " ".repeat(128 - base.len() - parameter.len())
    );
    assert_eq!(media.len(), 128);
    let body = b"id=tr_HEADER_BOUNDARY";
    let mut bytes = head("POST", PATH, "edge.invalid", Some(&media), &[], body.len());
    bytes.extend_from_slice(body);
    assert_eq!(exchange(edge.address, bytes).await.status, 200);
    let media = format!(
        "{base}{}{parameter}",
        " ".repeat(129 - base.len() - parameter.len())
    );
    assert_eq!(media.len(), 129);
    let mut bytes = head("POST", PATH, "edge.invalid", Some(&media), &[], body.len());
    bytes.extend_from_slice(body);
    assert_eq!(exchange(edge.address, bytes).await.status, 415);
    assert_eq!(upstream.records().len(), 1);
    edge.stop().await;
}

#[tokio::test]
async fn chunked_truncated_slow_and_cancelled_bodies_do_not_reach_upstream() {
    let upstream = Stub::start(Behavior::Reply(empty_200())).await;
    let edge = Server::start(router(configured(&values(upstream.address)))).await;
    let chunked_head = format!(
        "POST {PATH} HTTP/1.1\r\nHost: private-edge.invalid\r\nContent-Type: application/x-www-form-urlencoded\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    );
    let mut valid = chunked_head.as_bytes().to_vec();
    valid.extend_from_slice(b"3\r\nid=\r\nc\r\ntr_CHUNKED_X\r\n0\r\n\r\n");
    assert_eq!(exchange(edge.address, valid).await.status, 200);
    let body = format!("id=tr_{}", "x".repeat(1019));
    assert_eq!(body.len(), 1025);
    let mut oversized = chunked_head.as_bytes().to_vec();
    for part in [&body.as_bytes()[..512], &body.as_bytes()[512..]] {
        oversized.extend_from_slice(format!("{:x}\r\n", part.len()).as_bytes());
        oversized.extend_from_slice(part);
        oversized.extend_from_slice(b"\r\n");
    }
    oversized.extend_from_slice(b"0\r\n\r\n");
    assert_eq!(exchange(edge.address, oversized).await.status, 413);
    let mut truncated = TcpStream::connect(edge.address)
        .await
        .expect("truncated body connection");
    truncated
        .write_all(&head(
            "POST",
            PATH,
            "edge.invalid",
            Some("application/x-www-form-urlencoded"),
            &[],
            100,
        ))
        .await
        .expect("headers");
    truncated
        .write_all(b"id=tr_TRUNCATED")
        .await
        .expect("partial body");
    truncated.shutdown().await.expect("premature EOF");
    assert_eq!(read_response(&mut truncated).await.status, 400);
    let mut slow = TcpStream::connect(edge.address)
        .await
        .expect("slow body connection");
    let began = Instant::now();
    slow.write_all(&head(
        "POST",
        PATH,
        "edge.invalid",
        Some("application/x-www-form-urlencoded"),
        &[],
        100,
    ))
    .await
    .expect("headers");
    slow.write_all(b"i").await.expect("first body byte");
    tokio::time::sleep(Duration::from_millis(600)).await;
    slow.write_all(b"d")
        .await
        .expect("later incomplete body byte");
    let response = read_response(&mut slow).await;
    assert_eq!(response.status, 503);
    assert!(
        began.elapsed() < Duration::from_millis(1800),
        "the one-second whole-body budget does not reset for each byte"
    );
    let mut cancelled = TcpStream::connect(edge.address)
        .await
        .expect("cancelled body connection");
    cancelled
        .write_all(&head(
            "POST",
            PATH,
            "edge.invalid",
            Some("application/x-www-form-urlencoded"),
            &[],
            100,
        ))
        .await
        .expect("headers");
    cancelled.write_all(b"i").await.expect("partial body");
    drop(cancelled);
    assert_eq!(upstream.records().len(), 1);
    assert_eq!(post(edge.address, b"id=tr_AFTER_CANCEL").await.status, 200);
    assert_eq!(upstream.records().len(), 2);
    for (method, path, expected) in [
        ("GET", PATH, 405),
        ("POST", "/api/callbacks/mollie/extra", 404),
        ("POST", "/api/guest/session", 404),
        ("GET", "/health/live", 204),
    ] {
        let response = exchange(
            edge.address,
            head(method, path, "edge.invalid", None, &[], 0),
        )
        .await;
        assert_eq!(response.status, expected);
    }
    assert_eq!(upstream.records().len(), 2);
    edge.stop().await;
}

#[tokio::test]
async fn revision_and_referrer_headers_are_selected_by_server_configuration() {
    let upstream = Stub::start(Behavior::Reply(empty_200())).await;
    let revision = "0123456789abcdef0123456789abcdef01234567";
    let mut stage = values(upstream.address);
    stage.insert("APP_ENV".into(), "staging".into());
    stage.insert("CALLBACK_EDGE_BIND".into(), "0.0.0.0:45672".into());
    stage.insert("CALLBACK_EDGE_PROXY_IPS".into(), "10.1.2.3".into());
    stage.insert("PREVIEW_REVISION".into(), revision.into());
    let app = router(configured(&stage));
    // These unit requests inject server metadata solely to check response fields;
    // the separate actual-socket test proves which network peer is trusted.
    for (method, path, body, peer, expected) in [
        ("POST", PATH, "id=tr_STAGE_HEADER", Some("10.1.2.3:8"), 200),
        ("POST", PATH, "id=tr_", Some("10.1.2.3:8"), 400),
        ("POST", PATH, "id=tr_DENIED", Some("127.0.0.1:8"), 403),
        ("GET", "/health/live", "", None, 204),
        ("POST", "/unknown", "", None, 404),
    ] {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "x-piwas-preview-revision",
                "caller-untrusted-revision-marker",
            )
            .body(Body::from(body))
            .expect("response selection fixture");
        if let Some(peer) = peer {
            request.extensions_mut().insert(ConnectInfo(
                peer.parse::<SocketAddr>().expect("server metadata"),
            ));
        }
        let response = app
            .clone()
            .oneshot(request)
            .await
            .expect("server selected response");
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()["x-piwas-preview-revision"], revision);
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    }
    let mut request = Request::builder()
        .method("GET")
        .uri("/health/live")
        .header(
            "x-piwas-preview-revision",
            "caller-untrusted-revision-marker",
        )
        .body(Body::empty())
        .expect("unconfigured revision fixture");
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:8"
            .parse::<SocketAddr>()
            .expect("local server metadata"),
    ));
    let mut local = values(upstream.address);
    local.insert("PREVIEW_REVISION".into(), revision.into());
    let response = router(configured(&local))
        .oneshot(request)
        .await
        .expect("local response");
    assert!(
        response.headers().get("x-piwas-preview-revision").is_none(),
        "caller revision is never reflected"
    );
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    let request = upstream.records();
    assert_eq!(request.len(), 1);
    assert!(
        request[0].header("x-piwas-preview-revision").is_none(),
        "server response policy does not copy incoming revision into upstream requests"
    );
}

#[tokio::test]
async fn public_forwarder_boundary_rejects_unsafe_inputs_before_io_and_accepts_url_safe_server_tokens()
 {
    use commerce_infrastructure::callback_forwarder::{CallbackForwarder, ForwardError};
    let upstream = Stub::start(Behavior::Reply(empty_200())).await;
    let token = format!("{}-_", "a".repeat(30));
    assert_eq!(token.len(), 32);
    let origin = format!("http://{}/", upstream.address);
    let forwarder = CallbackForwarder::new(&origin, &token)
        .expect("canonical trailing slash and both URL-safe token symbols");
    assert_eq!(forwarder.forward(b"").await, Err(ForwardError::InvalidBody));
    assert_eq!(
        forwarder.forward(&vec![b'x'; 1025]).await,
        Err(ForwardError::InvalidBody)
    );
    for invalid in [
        format!("http://:private-password-marker@{}", upstream.address),
        "http://127.0.0.1:80".to_owned(),
        format!("http://127.000.000.001:{}", upstream.address.port()),
        format!("http://2130706433:{}", upstream.address.port()),
        format!("http://0x7f000001:{}", upstream.address.port()),
    ] {
        assert_eq!(
            CallbackForwarder::new(&invalid, &token).err(),
            Some(ForwardError::InvalidConfiguration),
            "password-only credentials and noncanonical numeric aliases cannot select an upstream"
        );
    }
    assert!(upstream.records().is_empty());
    assert_eq!(upstream.connections.load(Ordering::SeqCst), 0);
    forwarder
        .forward(b"id=tr_PUBLIC_FORWARDER_BOUNDARY")
        .await
        .expect("positive authenticated forwarding control");
    let requests = upstream.records();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, PATH);
    assert_eq!(requests[0].body, b"id=tr_PUBLIC_FORWARDER_BOUNDARY");
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {token}").as_str())
    );
}
