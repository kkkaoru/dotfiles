use super::await_write;

#[tokio::test]
async fn await_write_reports_a_closed_completion_channel() {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    drop(sender);

    let error = await_write(receiver)
        .await
        .expect_err("a dropped writer completion must fail");
    assert!(
        error.to_string().contains("writer stopped before flushing"),
        "unexpected error: {error}"
    );
}
