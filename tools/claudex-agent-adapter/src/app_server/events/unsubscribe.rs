use super::{EventQueue, Registry, ThreadRoute};

pub(super) fn unsubscribe(
    channels: &mut Registry,
    thread_id: &str,
    channel_id: u64,
    queue: &EventQueue,
) {
    let remove_route = match channels.get_mut(thread_id) {
        Some(route) => requeue_if_last_subscriber(route, channel_id, queue),
        None => false,
    };
    if remove_route {
        channels.remove(thread_id);
    }
}

fn requeue_if_last_subscriber(
    route: &mut ThreadRoute,
    channel_id: u64,
    queue: &EventQueue,
) -> bool {
    route
        .subscribers
        .retain(|(registered_id, _)| *registered_id != channel_id);
    if !route.subscribers.is_empty() {
        return false;
    }
    let mut state = queue.state.lock().expect("thread event queue poisoned");
    debug_assert!(route.backlog.events.is_empty());
    route.backlog = state.take_requeueable_backlog();
    route.backlog.events.is_empty()
}
