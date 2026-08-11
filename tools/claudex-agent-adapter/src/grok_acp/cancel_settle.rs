use anyhow::Result;

pub(super) fn settle_cancel_after_driver_loss(
    session_id: &str,
    error: anyhow::Error,
) -> Result<()> {
    let message = error.to_string();
    if message.contains("ACP driver is unavailable")
        || message.contains("ACP driver dropped its response")
    {
        tracing::info!(session_id, %error, "ACP cancel settled after driver loss");
        return Ok(());
    }
    Err(error)
}
