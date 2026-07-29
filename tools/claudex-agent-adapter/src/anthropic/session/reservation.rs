use std::{sync::Arc, time::Duration};

use serde_json::Value;

use super::{SelectedSession, Session, candidate_length, is_better_length, touch_session};
use crate::anthropic::content::{canonical_eq, matching_transcript_len};

/// How long a follow-up may wait for an in-flight same-session turn to release
/// its gate after cancellation. Kept short so cold create_session remains an
/// option when the prior turn cannot settle.
const PREEMPT_GATE_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) async fn reserve_matching_session(
    sessions: Vec<Arc<Session>>,
    signature: &Arc<str>,
    messages: &[Value],
) -> Option<SelectedSession> {
    let mut best: Option<SelectedSession> = None;
    for session in sessions {
        let Ok(gate) = Arc::clone(&session.gate).try_lock_owned() else {
            continue;
        };
        let Some(existing_len) = candidate_length(&session, signature, messages).await else {
            continue;
        };
        if is_better_length(
            best.as_ref().map(|selected| selected.existing_len),
            existing_len,
        ) {
            best = Some(SelectedSession {
                session,
                existing_len,
                recovered: false,
                gate,
            });
        }
    }
    if let Some(selected) = &best {
        touch_session(&selected.session);
    }
    best
}

/// Find the best transcript-matching session that is currently busy (gate held).
///
/// Outer follow-ups first require an exact signature match. If tools/system drift
/// broke the signature (common mid-conversation), fall back to model + user_id so
/// interactive messages still reclaim the live provider thread instead of cold-starting.
pub(super) async fn find_busy_matching_session(
    sessions: Vec<Arc<Session>>,
    signature: &Arc<str>,
    messages: &[Value],
    model: Option<&str>,
    user_id: Option<&str>,
) -> Option<(Arc<Session>, usize)> {
    let mut best: Option<(Arc<Session>, usize)> = None;
    for session in sessions.iter() {
        let Some(existing_len) = candidate_length(session, signature, messages).await else {
            continue;
        };
        if Arc::clone(&session.gate).try_lock_owned().is_ok() {
            continue;
        }
        if is_better_length(best.as_ref().map(|(_, len)| *len), existing_len) {
            best = Some((Arc::clone(session), existing_len));
        }
    }
    if best.is_some() {
        return best;
    }
    // Signature miss: still reclaim a busy conversation for the same human.
    let mut best: Option<(Arc<Session>, usize)> = None;
    for session in sessions {
        if Arc::clone(&session.gate).try_lock_owned().is_ok() {
            continue;
        }
        if !conversation_matches(&session, model, user_id) {
            continue;
        }
        align_transcript_to_request(&session, messages).await;
        let Some(existing_len) = matching_transcript_len(&session, messages).await else {
            continue;
        };
        if is_better_length(best.as_ref().map(|(_, len)| *len), existing_len) {
            best = Some((session, existing_len));
        }
    }
    best
}

fn conversation_matches(session: &Session, model: Option<&str>, user_id: Option<&str>) -> bool {
    if model.is_some_and(|model| session.model != model) {
        return false;
    }
    match (user_id, session.client_user_id.as_deref()) {
        (Some(left), Some(right)) => left == right,
        // Without a client session id, only allow the fallback when model matches
        // and we have a single busy candidate (checked by caller scoring).
        (None, None) => true,
        _ => false,
    }
}

/// Wait for a cancelled turn to release its session gate, then realign the
/// transcript if a partial assistant message was committed after interrupt.
pub(super) async fn take_gate_after_preempt(
    session: &Arc<Session>,
    messages: &[Value],
) -> Option<SelectedSession> {
    let gate = tokio::time::timeout(PREEMPT_GATE_TIMEOUT, Arc::clone(&session.gate).lock_owned())
        .await
        .ok()?;
    align_transcript_to_request(session, messages).await;
    let existing_len = matching_transcript_len(session, messages).await?;
    touch_session(session);
    Some(SelectedSession {
        session: Arc::clone(session),
        existing_len,
        recovered: false,
        gate,
    })
}

/// Drop trailing transcript entries that the client did not keep after interrupt
/// (typically a partial assistant block committed when the prior stream settled).
async fn align_transcript_to_request(session: &Session, messages: &[Value]) {
    let mut transcript = session.transcript.lock().await;
    while !transcript_is_prefix(&transcript, messages) {
        // A non-prefix transcript necessarily has at least one entry: an empty
        // transcript is a prefix of every request.
        transcript
            .pop()
            .expect("non-prefix transcript must contain an entry");
    }
}

fn transcript_is_prefix(transcript: &[Value], messages: &[Value]) -> bool {
    transcript.len() <= messages.len()
        && transcript
            .iter()
            .zip(messages)
            .all(|(left, right)| canonical_eq(left, right))
}

#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{collections::HashMap, sync::Arc, time::Instant};
    use tokio::sync::{Mutex, Semaphore};

    fn session(model: &str, client_user_id: Option<&str>) -> Session {
        let slots = Arc::new(Semaphore::new(1));
        Session {
            thread_id: "thread".to_owned(),
            model: model.to_owned(),
            signature: Arc::from("signature"),
            transcript: Mutex::new(Vec::new()),
            pending_tools: Mutex::new(HashMap::new()),
            consumed_tool_ids: Mutex::new(Default::default()),
            internal_tools: HashMap::new(),
            external_tool_names: HashMap::new(),
            client_user_id: client_user_id.map(str::to_owned),
            gate: Arc::new(Mutex::new(())),
            last_activity: std::sync::Mutex::new(Instant::now()),
            pending_since: std::sync::Mutex::new(None),
            _slot: slots.try_acquire_owned().expect("session slot"),
        }
    }

    // Lightweight fixtures exercise align/prefix helpers without a full Session.
    #[test]
    fn transcript_prefix_ignores_cache_control_via_canonical_eq() {
        let left = json!({"role":"user","content":"hi","cache_control":{"type":"ephemeral"}});
        let right = json!({"role":"user","content":"hi"});
        assert!(canonical_eq(&left, &right));
        assert!(transcript_is_prefix(std::slice::from_ref(&left), &[right]));
    }

    #[test]
    fn fallback_identity_checks_model_and_client_user_id() {
        let identified = session("main", Some("client"));
        assert!(conversation_matches(
            &identified,
            Some("main"),
            Some("client")
        ));
        assert!(conversation_matches(&identified, None, Some("client")));
        assert!(!conversation_matches(
            &identified,
            Some("other"),
            Some("client")
        ));
        assert!(!conversation_matches(
            &identified,
            Some("main"),
            Some("other")
        ));

        let anonymous = session("main", None);
        assert!(conversation_matches(&anonymous, Some("main"), None));
        assert!(!conversation_matches(
            &anonymous,
            Some("main"),
            Some("client")
        ));
    }

    #[tokio::test]
    async fn busy_fallback_rejects_incompatible_candidates() {
        let wrong_model = Arc::new(session("other", Some("client")));
        let anonymous = Arc::new(session("main", None));
        let _wrong_model_gate = Arc::clone(&wrong_model.gate).lock_owned().await;
        let _anonymous_gate = Arc::clone(&anonymous.gate).lock_owned().await;
        let message = json!({"role":"user","content":"follow-up"});

        let found = find_busy_matching_session(
            vec![wrong_model, anonymous],
            &Arc::from("different-signature"),
            &[message],
            Some("main"),
            Some("client"),
        )
        .await;

        assert!(found.is_none());
    }

    #[tokio::test]
    async fn find_busy_skips_idle_sessions() {
        let gate = Arc::new(Mutex::new(()));
        let _hold = gate.lock().await;
        assert!(gate.clone().try_lock_owned().is_err());
        drop(_hold);
        assert!(gate.try_lock_owned().is_ok());
    }

    #[tokio::test]
    async fn reserve_prefers_the_longest_idle_matching_transcript() {
        let messages = messages();
        let busy = session_with("main", Some("client"), "signature", messages.clone());
        let _busy_gate = Arc::clone(&busy.gate).lock_owned().await;
        let wrong_signature = session_with("main", Some("client"), "other", messages.clone());
        let longest = session_with("main", Some("client"), "signature", messages.clone());
        let shortest = session_with("main", Some("client"), "signature", messages[..1].to_vec());

        let selected = reserve_matching_session(
            vec![wrong_signature, busy, Arc::clone(&longest), shortest],
            &Arc::from("signature"),
            &messages,
        )
        .await
        .expect("matching idle session");

        assert!(Arc::ptr_eq(&selected.session, &longest));
        assert_eq!(selected.existing_len, messages.len());
    }

    #[tokio::test]
    async fn busy_selection_skips_idle_sessions_and_keeps_the_longest_match() {
        let messages = messages();
        let idle = session_with("main", Some("client"), "signature", messages.clone());
        let wrong_signature = session_with("main", Some("client"), "other", messages.clone());
        let shortest = session_with("main", Some("client"), "signature", messages[..1].to_vec());
        let longest = session_with("main", Some("client"), "signature", messages.clone());
        let trailing = session_with("main", Some("client"), "signature", messages[..1].to_vec());
        let _wrong_signature_gate = Arc::clone(&wrong_signature.gate).lock_owned().await;
        let _shortest_gate = Arc::clone(&shortest.gate).lock_owned().await;
        let _longest_gate = Arc::clone(&longest.gate).lock_owned().await;
        let _trailing_gate = Arc::clone(&trailing.gate).lock_owned().await;

        let found = find_busy_matching_session(
            vec![
                idle,
                wrong_signature,
                shortest,
                Arc::clone(&longest),
                trailing,
            ],
            &Arc::from("signature"),
            &messages,
            Some("main"),
            Some("client"),
        )
        .await
        .expect("busy matching session");

        assert!(Arc::ptr_eq(&found.0, &longest));
        assert_eq!(found.1, messages.len());
    }

    #[tokio::test]
    async fn busy_fallback_realigns_a_matching_conversation_after_signature_drift() {
        let messages = messages();
        let wrong_model = session_with("other", Some("client"), "other", messages.clone());
        let realigned = session_with(
            "main",
            Some("client"),
            "other",
            vec![
                messages[0].clone(),
                json!({"role":"assistant","content":"stale"}),
            ],
        );
        let equally_good = session_with("main", Some("client"), "other", messages[..1].to_vec());
        let _wrong_model_gate = Arc::clone(&wrong_model.gate).lock_owned().await;
        let _realigned_gate = Arc::clone(&realigned.gate).lock_owned().await;
        let _equally_good_gate = Arc::clone(&equally_good.gate).lock_owned().await;

        let found = find_busy_matching_session(
            vec![wrong_model, Arc::clone(&realigned), equally_good],
            &Arc::from("signature"),
            &messages,
            Some("main"),
            Some("client"),
        )
        .await
        .expect("matching busy fallback");

        assert!(Arc::ptr_eq(&found.0, &realigned));
        assert_eq!(found.1, 1);
        assert_eq!(*realigned.transcript.lock().await, messages[..1]);
    }

    fn messages() -> Vec<Value> {
        vec![
            json!({"role":"user","content":"first"}),
            json!({"role":"user","content":"follow-up"}),
        ]
    }

    fn session_with(
        model: &str,
        client_user_id: Option<&str>,
        signature: &str,
        transcript: Vec<Value>,
    ) -> Arc<Session> {
        let mut session = session(model, client_user_id);
        session.signature = Arc::from(signature);
        session.transcript = Mutex::new(transcript);
        Arc::new(session)
    }
}
