use super::*;

#[tokio::test]
async fn accept_error_backoffs_and_returns_none() {
    let error = std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "test accept error");
    assert!(accept_or_backoff(Err(error)).await.is_none());
}
