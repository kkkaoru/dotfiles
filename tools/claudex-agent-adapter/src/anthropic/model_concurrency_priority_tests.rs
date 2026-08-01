use std::time::Duration;

use super::ModelConcurrency;

#[tokio::test]
async fn interactive_turn_uses_a_reserved_slot_while_background_is_busy() {
    let registry = ModelConcurrency::new(vec![("priority".to_owned(), 2)]);
    let background = registry
        .ticket("priority", Some(2))
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let interactive = tokio::time::timeout(
        Duration::from_millis(50),
        registry
            .ticket("priority", Some(2))
            .unwrap()
            .acquire_for(true),
    )
    .await
    .expect("interactive request must use the reserved slot")
    .expect("interactive slot must be available");
    assert_eq!(registry.snapshot()["priority"].active, 2);
    drop((background, interactive));
}
