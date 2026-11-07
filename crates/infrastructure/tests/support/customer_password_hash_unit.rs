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
