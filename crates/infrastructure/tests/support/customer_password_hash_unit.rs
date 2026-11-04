use super::*;

#[tokio::test]
async fn capacity_permit_is_returned_when_dropped() {
    let work = Semaphore::new(1);
    let permit = acquire_capacity(&work).await.expect("available capacity");
    assert_eq!(work.available_permits(), 0);
    drop(permit);
    assert_eq!(work.available_permits(), 1);
    assert!(acquire_capacity(&work).await.is_ok());
}

#[tokio::test]
async fn exhausted_capacity_times_out() {
    let work = Semaphore::new(0);
    assert!(matches!(
        acquire_capacity(&work).await,
        Err(PasswordError::Unavailable)
    ));
}

#[tokio::test]
async fn closed_capacity_fails_even_with_available_permits() {
    let work = Semaphore::new(1);
    work.close();
    assert!(matches!(
        acquire_capacity(&work).await,
        Err(PasswordError::Unavailable)
    ));
}

#[tokio::test]
async fn verification_uses_the_exact_password_and_never_accepts_the_dummy() {
    let decomposed = "  cafe\u{301} phrase long  ";
    let composed = "  café phrase long  ";
    let encoded = hash_password(decomposed).await.expect("current hash");
    assert_eq!(verify_password(composed, Some(&encoded)).await, Ok(true));
    assert_eq!(
        verify_password("café phrase long", Some(&encoded)).await,
        Ok(false)
    );
    assert_eq!(verify_password(composed, None).await, Ok(false));
}

#[tokio::test]
async fn verification_rejects_corrupt_profile_encoding_and_field_widths() {
    let password = "a fixture password long enough";
    let salt = "A".repeat(22);
    let digest = "A".repeat(43);
    let wrong_parameters = DUMMY_PHC.replace("t=2", "t=1");
    let malformed_base64 = format!("{PHC_PREFIX}!{}${digest}", "A".repeat(21));
    let short_salt = format!("{PHC_PREFIX}{}${digest}", "A".repeat(20));
    let short_digest = format!("{PHC_PREFIX}{salt}${}", "A".repeat(42));
    let oversized = format!("{DUMMY_PHC}A");
    for malformed in [
        wrong_parameters,
        malformed_base64,
        short_salt,
        short_digest,
        oversized,
    ] {
        assert_eq!(
            verify_password(password, Some(&malformed)).await,
            Err(PasswordError::Unavailable)
        );
    }
    assert_eq!(verify_password(password, None).await, Ok(false));
}
