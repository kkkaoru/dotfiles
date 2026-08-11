use std::{sync::Mutex, time::Duration};

use anyhow::anyhow;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::bytes::Bytes;

use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};
use crate::anthropic::model_concurrency::is_concurrency_admission_timeout;
use crate::anthropic::stream::builder::SegmentBuilder;
use crate::anthropic::{Bridge, MessagesRequest};
use crate::provider_config::ModelCatalog;

use super::*;

const CONCURRENCY_WAIT_TIMEOUT_ENV: &str = "CLAUDEX_MODEL_CONCURRENCY_WAIT_TIMEOUT_MS";

static CONCURRENCY_TIMEOUT_ENV: Mutex<()> = Mutex::new(());

const QWEN_CLOUD: &str = "qwen3.8-max-preview";
const CURSOR_AUTO: &str = "auto";

fn dummy_request(model: &str) -> MessagesRequest {
    MessagesRequest {
        model: model.to_owned(),
        system: json!(null),
        messages: vec![],
        tools: vec![],
        stream: true,
        output_config: json!({}),
        metadata: json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

fn qwen_cursor_bridge() -> Bridge {
    let mut qwen = BackendRoute::new(QWEN_CLOUD, BackendKind::ConfiguredAcp);
    qwen.max_concurrency = Some(3);
    let mut cursor = BackendRoute::new(CURSOR_AUTO, BackendKind::ConfiguredAcp);
    cursor.max_concurrency = Some(3);
    let backend = AgentBackend::spawn_routes(&[qwen, cursor]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![
            crate::provider_config::WorkerRoute::new("claudex-qwen", QWEN_CLOUD, "high"),
            crate::provider_config::WorkerRoute::new("claudex-cursor", CURSOR_AUTO, "high"),
        ])
        .expect("workers");
    Bridge::new_with_backend(backend, QWEN_CLOUD.to_owned()).with_model_catalog(catalog)
}

async fn saturate_qwen_subagent_slots(
    bridge: &Bridge,
) -> Vec<crate::anthropic::model_concurrency::ModelPermit> {
    let mut permits = Vec::new();
    for _ in 0..2 {
        permits.push(
            bridge
                .model_concurrency
                .ticket(QWEN_CLOUD, Some(3))
                .expect("qwen ticket")
                .acquire_for(false)
                .await
                .expect("qwen slot"),
        );
    }
    assert!(bridge.model_concurrency.is_subagent_at_capacity(QWEN_CLOUD));
    permits
}

#[tokio::test]
async fn acquire_prepared_permit_returns_none_when_no_ticket() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    let mut request = dummy_request("main");
    let mut effort = None;

    let result = bridge
        .acquire_prepared_permit(&mut request, &mut effort, None, false)
        .await
        .expect("no-ticket acquire");
    assert!(result.is_none(), "no ticket should yield None permit");
}

#[tokio::test]
async fn acquire_prepared_permit_succeeds_for_a_free_ticket() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    let ticket = bridge
        .model_concurrency
        .ticket("test-model", Some(1))
        .expect("ticket");
    let mut request = dummy_request("test-model");
    let mut effort = None;

    let result = bridge
        .acquire_prepared_permit(&mut request, &mut effort, Some(ticket), false)
        .await
        .expect("ticket acquire");
    assert!(result.is_some(), "free ticket should yield a permit");
}

#[tokio::test]
async fn reticket_saturated_subagent_rewrites_model_and_ticket() {
    let bridge = qwen_cursor_bridge();
    let _permits = saturate_qwen_subagent_slots(&bridge).await;
    let mut request = dummy_request(QWEN_CLOUD);
    let mut effort = Some("high".to_owned());
    let mut ticket = bridge.model_concurrency.ticket(QWEN_CLOUD, Some(3));
    bridge.reticket_saturated_subagent(&mut request, &mut effort, &mut ticket, false);
    assert_eq!(request.model, QWEN_CLOUD, "non-subagent must keep model");

    bridge.reticket_saturated_subagent(&mut request, &mut effort, &mut ticket, true);
    assert_eq!(request.model, CURSOR_AUTO);
    assert!(
        ticket.is_some(),
        "rewritten model keeps a concurrency ticket"
    );
}

#[tokio::test]
async fn acquire_prepared_permit_retickets_subagent_after_admission_timeout() {
    // SAFETY: serialized by CONCURRENCY_TIMEOUT_ENV for this test process.
    {
        let _env = CONCURRENCY_TIMEOUT_ENV
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var(CONCURRENCY_WAIT_TIMEOUT_ENV, "1");
        }
    }

    let bridge = qwen_cursor_bridge();
    let _permits = saturate_qwen_subagent_slots(&bridge).await;
    let ticket = bridge
        .model_concurrency
        .ticket(QWEN_CLOUD, Some(3))
        .expect("saturated ticket");
    let mut request = dummy_request(QWEN_CLOUD);
    let mut effort = Some("high".to_owned());

    let result = bridge
        .acquire_prepared_permit(&mut request, &mut effort, Some(ticket), true)
        .await
        .expect("sibling reticket after timeout");
    assert!(result.is_some());
    assert_eq!(request.model, CURSOR_AUTO);

    let _env = CONCURRENCY_TIMEOUT_ENV
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    unsafe {
        std::env::remove_var(CONCURRENCY_WAIT_TIMEOUT_ENV);
    }
}

#[tokio::test]
async fn acquire_prepared_permit_errors_when_timeout_has_no_sibling() {
    {
        let _env = CONCURRENCY_TIMEOUT_ENV
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            std::env::set_var(CONCURRENCY_WAIT_TIMEOUT_ENV, "1");
        }
    }

    let mut qwen = BackendRoute::new(QWEN_CLOUD, BackendKind::ConfiguredAcp);
    qwen.max_concurrency = Some(3);
    let backend = AgentBackend::spawn_routes(&[qwen]);
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-qwen",
            QWEN_CLOUD,
            "high",
        )])
        .expect("qwen worker");
    let bridge =
        Bridge::new_with_backend(backend, QWEN_CLOUD.to_owned()).with_model_catalog(catalog);
    let _permits = saturate_qwen_subagent_slots(&bridge).await;
    let ticket = bridge
        .model_concurrency
        .ticket(QWEN_CLOUD, Some(3))
        .expect("saturated ticket");
    let mut request = dummy_request(QWEN_CLOUD);
    let mut effort = Some("high".to_owned());

    let error = match bridge
        .acquire_prepared_permit(&mut request, &mut effort, Some(ticket), true)
        .await
    {
        Ok(_) => panic!("no sibling after timeout"),
        Err(error) => error,
    };
    assert!(is_concurrency_admission_timeout(&error));

    let _env = CONCURRENCY_TIMEOUT_ENV
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    unsafe {
        std::env::remove_var(CONCURRENCY_WAIT_TIMEOUT_ENV);
    }
}

#[tokio::test]
async fn fail_prepared_stream_skips_exhaustion_for_concurrency_timeout() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned());
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, std::convert::Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    let error = anyhow!(
        "model `{QWEN_CLOUD}` concurrency model admission timed out after {:?}",
        Duration::from_millis(1)
    );
    bridge
        .fail_prepared_stream(&sender, &mut builder, error, QWEN_CLOUD)
        .await;
    drop(sender);
    assert!(
        !bridge.subagent_provider_is_exhausted(QWEN_CLOUD),
        "admission timeout must not cool down the provider"
    );
    assert!(receiver.recv().await.is_some(), "error frame still streams");
}
