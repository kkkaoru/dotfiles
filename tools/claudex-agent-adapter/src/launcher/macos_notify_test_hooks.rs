use super::*;

#[cfg(test)]
pub(crate) struct TestEvents;

#[cfg(test)]
impl TestEvents {
    pub(crate) fn capture() -> Self {
        take_events();
        Self
    }

    pub(crate) fn take(&self) -> Vec<Event> {
        take_events()
    }
}

#[cfg(test)]
impl Drop for TestEvents {
    fn drop(&mut self) {
        take_events();
    }
}

#[cfg(test)]
pub(crate) struct TestSpawn;

#[cfg(test)]
impl TestSpawn {
    pub(crate) fn arm(spawn: fn(&Notification) -> std::io::Result<ExitStatus>) -> Self {
        take_events();
        TEST_SPAWN.with(|cell| cell.set(Some(spawn)));
        Self
    }

    pub(crate) fn take_events(&self) -> Vec<Event> {
        take_events()
    }
}

#[cfg(test)]
impl Drop for TestSpawn {
    fn drop(&mut self) {
        TEST_SPAWN.with(|cell| cell.set(None));
        take_events();
    }
}
