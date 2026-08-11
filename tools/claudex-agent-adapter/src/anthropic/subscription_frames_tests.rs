use super::*;

#[tokio::test]
async fn marks_missing_provider_environment_as_non_retryable() {
    let (sender, mut receiver) = mpsc::channel(1);

    send_subscription_error(
        &sender,
        anyhow::anyhow!("Missing environment variable: SAKANA_AI_API_KEY"),
    )
    .await;

    let frame = receiver
        .recv()
        .await
        .expect("error frame")
        .expect("infallible frame");
    assert!(String::from_utf8_lossy(&frame).contains("invalid_request_error"));
}
