use std::collections::VecDeque;

use super::super::{MessagesRequest, RequestIdentity};
use super::{
    SchemaIdentity, StoredSchema, ToolSchemaCache, bound_entries, retain_fresh, unix_seconds,
};

impl ToolSchemaCache {
    pub(in crate::anthropic) fn restore_or_remember(
        &self,
        identity: &RequestIdentity,
        request: &mut MessagesRequest,
        tools_were_provided: bool,
    ) {
        self.restore_or_remember_at(identity, request, tools_were_provided, unix_seconds());
    }

    pub(super) fn restore_or_remember_at(
        &self,
        identity: &RequestIdentity,
        request: &mut MessagesRequest,
        tools_were_provided: bool,
        now: u64,
    ) {
        let Some(identity) = SchemaIdentity::from_request(identity) else {
            return;
        };
        let mut entries = self.entries.lock().expect("tool schema cache poisoned");
        retain_fresh(&mut entries, now);
        if !tools_were_provided {
            self.restore_cached_schema(&mut entries, &identity, request, now);
            return;
        }
        // An explicit empty array is an authoritative zero-capability request.
        // It neither restores nor mutates a previously remembered schema.
        if request.tools.is_empty() {
            return;
        }

        entries.retain(|entry| entry.identity != identity);
        let stored = self.persist(
            StoredSchema {
                identity,
                tools: request.tools.clone(),
                updated_unix_seconds: now,
                accessed_unix_seconds: now,
            },
            now,
        );
        entries.push_back(stored);
        bound_entries(&mut entries);
    }

    fn restore_cached_schema(
        &self,
        entries: &mut VecDeque<StoredSchema>,
        identity: &SchemaIdentity,
        request: &mut MessagesRequest,
        now: u64,
    ) {
        if !request.tools.is_empty() {
            return;
        }
        let Some(index) = entries
            .iter()
            .rposition(|entry| entry.identity == *identity)
        else {
            return;
        };
        let mut stored = entries.remove(index).expect("matched schema entry");
        stored.accessed_unix_seconds = now;
        let stored = self.persist(stored, now);
        request.tools.clone_from(&stored.tools);
        entries.push_back(stored);
        bound_entries(entries);
    }

    fn persist(&self, stored: StoredSchema, now: u64) -> StoredSchema {
        match &self.store {
            Some(store) => store.save(stored, now),
            None => stored,
        }
    }
}
