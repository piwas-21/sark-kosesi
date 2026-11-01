//! Fixed loopback target and redacted server credential controls; no external I/O.
use super::*;
const TOKEN: &str = "synthetic_callback_preview_token_1234567890";
#[test]
fn callback_forwarder_has_fixed_path_sensitive_authorization_and_no_credential_debug_output() {
    for origin in [
        "http://127.0.0.1:45671",
        "http://127.0.0.1:45671/",
        "http://[::1]:45671",
        "http://127.0.0.2:45671",
    ] {
        let client = CallbackForwarder::new(origin, TOKEN).expect("literal loopback target");
        assert_eq!(client.target.path(), "/api/callbacks/mollie");
        assert!(client.authorization.is_sensitive());
        assert_eq!(
            client.authorization.to_str().expect("known fixtureheader"),
            format!("Bearer {TOKEN}")
        );
        assert!(!format!("{:?}", client.authorization).contains(TOKEN));
    }
}
#[test]
fn callback_target_and_credential_validation_fail_closed_without_an_outbound_operation() {
    for origin in [
        "",
        "not a url",
        "https://127.0.0.1:45671",
        "http://localhost:45671",
        "http://192.168.1.2:45671",
        "http://0.0.0.0:45671",
        "http://example.test",
        "http://127.0.0.1:0",
        "http://user@127.0.0.1:45671",
        "http://user:password@127.0.0.1:45671",
        "http://127.0.0.1:45671/another",
        "http://127.0.0.1:45671/?query=1",
        "http://127.0.0.1:45671/#fragment",
        "http://0177.0.0.1:45671",
        "http://127.0.0.1:45671/ ",
    ] {
        assert!(
            matches!(
                CallbackForwarder::new(origin, TOKEN),
                Err(ForwardError::InvalidConfiguration)
            ),
            "{origin}"
        );
    }
    for token in [
        "".to_owned(),
        "x".repeat(31),
        "x".repeat(129),
        format!("{TOKEN}\n"),
        format!("{TOKEN}!"),
        format!("{TOKEN}é"),
    ] {
        assert!(matches!(
            CallbackForwarder::new("http://127.0.0.1:45671", &token),
            Err(ForwardError::InvalidConfiguration)
        ));
    }
    for token in ["x".repeat(32), "x".repeat(128), "-_".repeat(16)] {
        assert!(CallbackForwarder::new("http://127.0.0.1:45671", &token).is_ok());
    }
}
#[tokio::test]
async fn callback_forwarder_rejects_empty_and_oversized_body_before_connecting() {
    let client = CallbackForwarder::new("http://127.0.0.1:1", TOKEN).expect("valid closedtarget");
    assert_eq!(client.forward(&[]).await, Err(ForwardError::InvalidBody));
    assert_eq!(
        client.forward(&vec![b'x'; 1025]).await,
        Err(ForwardError::InvalidBody)
    );
}
