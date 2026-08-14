use serde_json::json;
use std::path::Path;

use super::persist::{StoreLock, create_private_directory, parent_directory, write_private};
use super::*;

fn request(tools: Vec<Value>) -> MessagesRequest {
    serde_json::from_value(json!({
        "model":"main",
        "messages":[{"role":"user","content":"synthetic task"}],
        "tools":tools
    }))
    .expect("request")
}

fn omitted_request() -> MessagesRequest {
    serde_json::from_value(json!({
        "model":"main",
        "messages":[{"role":"user","content":"synthetic task"}]
    }))
    .expect("request")
}

fn identity(session: &str, agent: Option<&str>, parent: Option<&str>) -> RequestIdentity {
    RequestIdentity::new(
        Some(session.to_owned()),
        agent.map(str::to_owned),
        parent.map(str::to_owned),
    )
}

#[test]
fn persistent_round_trip_restores_the_exact_received_schema() {
    let root = tempfile::tempdir().expect("schema cache fixture");
    let path = root.path().join("schemas.json");
    let expected = vec![json!({
        "name":"SyntheticTool",
        "description":"fixture schema only",
        "input_schema":{
            "type":"object",
            "properties":{"value":{"type":"string","enum":["a","b"]}},
            "required":["value"],
            "additionalProperties":false
        }
    })];
    let first = ToolSchemaCache::with_store(path.clone());
    let mut initial = request(expected.clone());
    first.restore_or_remember(&identity("session-a", None, None), &mut initial, true);

    let restored = ToolSchemaCache::with_store(path);
    let mut resumed = omitted_request();
    restored.restore_or_remember(&identity("session-a", None, None), &mut resumed, false);
    assert_eq!(resumed.tools, expected);
}

#[test]
fn repeated_resumes_extend_access_beyond_the_original_two_hour_ttl() {
    let root = tempfile::tempdir().expect("schema cache fixture");
    let path = root.path().join("schemas.json");
    let expected = vec![json!({"name":"LongTask","input_schema":{"type":"object"}})];
    let owner = identity("session-long", None, None);
    let started_at = unix_seconds();
    let interval = MAX_AGE_SECONDS - 1;
    let cache = ToolSchemaCache::with_store(path.clone());
    cache.restore_or_remember_at(&owner, &mut request(expected.clone()), true, started_at);

    let mut first_resume = omitted_request();
    cache.restore_or_remember_at(&owner, &mut first_resume, false, started_at + interval);
    assert_eq!(first_resume.tools, expected);
    drop(cache);

    let restored = ToolSchemaCache::with_store(path);
    let mut second_resume = omitted_request();
    restored.restore_or_remember_at(&owner, &mut second_resume, false, started_at + 2 * interval);
    assert_eq!(second_resume.tools, expected);
}

#[test]
fn explicit_empty_tools_neither_restore_nor_replace_the_cached_schema() {
    let cache = ToolSchemaCache::default();
    let expected = vec![json!({"name":"Allowed","input_schema":{"type":"object"}})];
    let owner = identity("session-explicit-empty", None, None);
    cache.restore_or_remember(&owner, &mut request(expected.clone()), true);

    let mut explicit_empty = request(Vec::new());
    cache.restore_or_remember(&owner, &mut explicit_empty, true);
    assert!(explicit_empty.tools.is_empty());

    let mut omitted = omitted_request();
    cache.restore_or_remember(&owner, &mut omitted, false);
    assert_eq!(omitted.tools, expected);
}

#[test]
fn stale_daemon_snapshot_does_not_replace_a_newer_schema_generation() {
    let root = tempfile::tempdir().expect("schema cache fixture");
    let path = root.path().join("schemas.json");
    let owner = identity("session-shared", None, None);
    let other = identity("session-other", None, None);
    let old = vec![json!({"name":"Old","input_schema":{"type":"object"}})];
    let new = vec![json!({"name":"New","input_schema":{"type":"object"}})];
    let other_tools = vec![json!({"name":"Other","input_schema":{"type":"object"}})];
    let started_at = unix_seconds();

    let first = ToolSchemaCache::with_store(path.clone());
    first.restore_or_remember_at(&owner, &mut request(old), true, started_at);
    let stale = ToolSchemaCache::with_store(path.clone());
    first.restore_or_remember_at(&owner, &mut request(new.clone()), true, started_at + 1);
    stale.restore_or_remember_at(
        &other,
        &mut request(other_tools.clone()),
        true,
        started_at + 2,
    );

    let merged = ToolSchemaCache::with_store(path);
    let mut owner_resume = omitted_request();
    merged.restore_or_remember_at(&owner, &mut owner_resume, false, started_at + 3);
    assert_eq!(owner_resume.tools, new);
    let mut other_resume = omitted_request();
    merged.restore_or_remember_at(&other, &mut other_resume, false, started_at + 3);
    assert_eq!(other_resume.tools, other_tools);
}

#[test]
fn cache_isolates_sessions_agents_and_parent_lineage() {
    let cache = ToolSchemaCache::default();
    let expected = vec![json!({"name":"SyntheticTool","input_schema":{"type":"object"}})];
    let owner = identity("session-a", Some("agent-a"), Some("parent-a"));
    cache.restore_or_remember(&owner, &mut request(expected.clone()), true);

    for other in [
        identity("session-b", Some("agent-a"), Some("parent-a")),
        identity("session-a", Some("agent-b"), Some("parent-a")),
        identity("session-a", Some("agent-a"), Some("parent-b")),
    ] {
        let mut request = omitted_request();
        cache.restore_or_remember(&other, &mut request, false);
        assert!(request.tools.is_empty());
    }

    let mut matching = omitted_request();
    cache.restore_or_remember(&owner, &mut matching, false);
    assert_eq!(matching.tools, expected);
}

#[test]
fn empty_or_unidentified_cache_never_adds_capabilities() {
    let cache = ToolSchemaCache::default();
    let mut fresh = omitted_request();
    cache.restore_or_remember(&identity("fresh", None, None), &mut fresh, false);
    assert!(fresh.tools.is_empty());

    let mut unidentified = omitted_request();
    cache.restore_or_remember(&RequestIdentity::default(), &mut unidentified, false);
    assert!(unidentified.tools.is_empty());
}

#[test]
fn ignores_malformed_incompatible_and_expired_persisted_entries() {
    let root = tempfile::tempdir().expect("schema cache fixture");
    let path = root.path().join("schemas.json");
    std::fs::write(&path, b"not json").expect("malformed cache");
    assert!(
        ToolSchemaCache::with_store(path.clone())
            .entries
            .lock()
            .unwrap()
            .is_empty()
    );

    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({"version":99,"entries":[]}))
            .expect("incompatible cache"),
    )
    .expect("write incompatible cache");
    assert!(
        ToolSchemaCache::with_store(path.clone())
            .entries
            .lock()
            .unwrap()
            .is_empty()
    );

    let identity = identity("expired", None, None);
    let expired = StoredSchema {
        identity: SchemaIdentity::from_request(&identity).expect("identity"),
        tools: vec![json!({"name":"Expired"})],
        updated_unix_seconds: 1,
        accessed_unix_seconds: 1,
    };
    std::fs::write(
        &path,
        serde_json::to_vec(&StoredSchemas {
            version: CACHE_VERSION,
            entries: vec![expired],
        })
        .expect("expired cache"),
    )
    .expect("write expired cache");
    let cache = ToolSchemaCache::with_store(path);
    assert!(cache.entries.lock().unwrap().is_empty());
}

#[test]
fn bounds_and_merges_cache_generations_at_equal_and_older_timestamps() {
    let root = tempfile::tempdir().expect("schema cache fixture");
    let path = root.path().join("schemas.json");
    let owner = identity("merge", None, None);
    let first = vec![json!({"name":"first"})];
    let now = unix_seconds();
    ToolSchemaCache::with_store(path.clone()).restore_or_remember_at(
        &owner,
        &mut request(first.clone()),
        true,
        now,
    );

    let stale = ToolSchemaCache::with_store(path.clone());
    stale.restore_or_remember_at(
        &owner,
        &mut request(vec![json!({"name":"older"})]),
        true,
        now - 1,
    );
    let restored_cache = ToolSchemaCache::with_store(path);
    let mut restored = omitted_request();
    restored_cache.restore_or_remember_at(&owner, &mut restored, false, now);
    assert_eq!(restored.tools, first);

    let cache = ToolSchemaCache::default();
    for index in 0..(MAX_ENTRIES + 2) {
        cache.restore_or_remember_at(
            &identity(&format!("session-{index}"), None, None),
            &mut request(vec![json!({"name":format!("tool-{index}")})]),
            true,
            100 + index as u64,
        );
    }
    assert!(cache.entries.lock().unwrap().len() <= MAX_ENTRIES);
}

#[test]
fn ignores_nonempty_resume_tools_and_invalid_cached_entries() {
    let cache = ToolSchemaCache::default();
    let owner = identity("resume-tools", None, None);
    cache.restore_or_remember(
        &owner,
        &mut request(vec![json!({"name":"remembered"})]),
        true,
    );
    let mut explicit = request(vec![json!({"name":"explicit"})]);
    cache.restore_or_remember(&owner, &mut explicit, false);
    assert_eq!(explicit.tools, vec![json!({"name":"explicit"})]);

    let root = tempfile::tempdir().expect("schema cache fixture");
    let path = root.path().join("schemas.json");
    let invalid = StoredSchemas {
        version: CACHE_VERSION,
        entries: vec![
            StoredSchema {
                identity: SchemaIdentity {
                    session_id: String::new(),
                    agent_id: None,
                    parent_agent_id: None,
                },
                tools: vec![json!({"name":"empty-session"})],
                updated_unix_seconds: unix_seconds(),
                accessed_unix_seconds: unix_seconds(),
            },
            StoredSchema {
                identity: SchemaIdentity {
                    session_id: "empty-tools".to_owned(),
                    agent_id: None,
                    parent_agent_id: None,
                },
                tools: Vec::new(),
                updated_unix_seconds: unix_seconds(),
                accessed_unix_seconds: unix_seconds(),
            },
        ],
    };
    std::fs::write(&path, serde_json::to_vec(&invalid).expect("invalid cache"))
        .expect("write invalid cache");
    assert!(
        ToolSchemaCache::with_store(path)
            .entries
            .lock()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn falls_back_when_the_schema_store_parent_cannot_be_created() {
    let root = tempfile::tempdir().expect("schema cache fixture");
    let parent = root.path().join("not-a-directory");
    std::fs::write(&parent, "occupied").expect("occupied parent");
    let path = parent.join("schemas.json");
    let cache = ToolSchemaCache::with_store(path);
    let owner = identity("fallback", None, None);
    let tools = vec![json!({"name":"fallback"})];
    cache.restore_or_remember(&owner, &mut request(tools.clone()), true);
    let mut resumed = omitted_request();
    cache.restore_or_remember(&owner, &mut resumed, false);
    assert_eq!(resumed.tools, tools);
}

#[test]
fn persistence_io_helpers_cover_relative_paths_private_writes_and_locks() {
    let root = tempfile::tempdir().expect("schema cache fixture");
    let nested = root.path().join("nested");
    let cache_path = nested.join("schemas.json");

    assert_eq!(parent_directory(Path::new("schemas.json")), Path::new("."));
    assert_eq!(parent_directory(&cache_path), nested.as_path());
    create_private_directory(&nested).expect("create private schema directory");

    let payload_path = nested.join("payload");
    write_private(&payload_path, b"private payload").expect("write private payload");
    assert_eq!(
        std::fs::read(&payload_path).expect("read private payload"),
        b"private payload"
    );

    let lock = StoreLock::acquire(&cache_path).expect("acquire schema cache lock");
    assert!(cache_path.with_extension("lock").exists());
    drop(lock);
}
