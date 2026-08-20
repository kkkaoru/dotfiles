use super::routed_thread;

#[test]
fn splits_a_routed_thread_id_into_index_and_raw_id() {
    assert_eq!(routed_thread("3:session"), (3, "session"));
    assert_eq!(routed_thread("0:raw"), (0, "raw"));
}
