use serde_json::Value;

use super::SubscriptionFailureKind;

const EVIDENCE_POINTERS: [&str; 6] = [
    "/code",
    "/status",
    "/subtype",
    "/type",
    "/error/code",
    "/error/type",
];
const CONTEXT_LIMIT_MARKERS: [&str; 6] = [
    "context limit",
    "context window",
    "context_length",
    "prompt is too long",
    "too many tokens",
    "max_tokens",
];
const AUTHENTICATION_MARKERS: [&str; 7] = [
    "authentication",
    "unauthorized",
    "forbidden",
    "invalid api key",
    "oauth",
    "not logged in",
    "login required",
];
const UPSTREAM_TRANSIENT_MARKERS: [&str; 10] = [
    "bad gateway",
    "gateway timeout",
    "internal server error",
    "service unavailable",
    "temporarily unavailable",
    "overloaded",
    "rate_limit",
    "rate limit",
    "too many requests",
    "api_error",
];
const CONFIGURATION_MARKERS: [&str; 6] = [
    "invalid_request",
    "invalid request",
    "model not found",
    "not a model",
    "does not recognize",
    "configuration",
];

pub(super) fn classify_failure(value: Option<&Value>, diagnostic: &str) -> SubscriptionFailureKind {
    let evidence = failure_evidence(value, diagnostic);
    let explicit_status = failure_status(value, Some(diagnostic));
    if explicit_status == Some(413) || contains_any(&evidence, &CONTEXT_LIMIT_MARKERS) {
        return SubscriptionFailureKind::ContextLimit;
    }
    if matches!(explicit_status, Some(401 | 403))
        || contains_any(&evidence, &AUTHENTICATION_MARKERS)
    {
        return SubscriptionFailureKind::Authentication;
    }
    if matches!(explicit_status, Some(429 | 500 | 502 | 503 | 504))
        || contains_any(&evidence, &UPSTREAM_TRANSIENT_MARKERS)
    {
        return SubscriptionFailureKind::UpstreamTransient;
    }
    if matches!(explicit_status, Some(400 | 404))
        || contains_any(&evidence, &CONFIGURATION_MARKERS)
        || (evidence.contains("provider") && evidence.contains("not found"))
    {
        return SubscriptionFailureKind::Configuration;
    }
    SubscriptionFailureKind::Protocol
}

pub(super) fn status_hint(
    kind: SubscriptionFailureKind,
    value: Option<&Value>,
    diagnostic: Option<&str>,
) -> u16 {
    let explicit = failure_status(value, diagnostic);
    match kind {
        SubscriptionFailureKind::UpstreamTransient => explicit.unwrap_or(502),
        SubscriptionFailureKind::Authentication => match explicit {
            Some(403) => 403,
            _ => 401,
        },
        SubscriptionFailureKind::Configuration => 400,
        SubscriptionFailureKind::ContextLimit => 413,
        SubscriptionFailureKind::LocalProcess
        | SubscriptionFailureKind::LocalTimeout
        | SubscriptionFailureKind::EmptyProcessOutput
        | SubscriptionFailureKind::Protocol => 424,
    }
}

fn failure_evidence(value: Option<&Value>, diagnostic: &str) -> String {
    let mut evidence = diagnostic.to_ascii_lowercase();
    for field in EVIDENCE_POINTERS
        .iter()
        .filter_map(|pointer| value.and_then(|document| document.pointer(pointer)))
    {
        evidence.push(' ');
        evidence.push_str(&field.to_string().to_ascii_lowercase());
    }
    evidence
}

fn failure_status(value: Option<&Value>, diagnostic: Option<&str>) -> Option<u16> {
    value
        .and_then(|value| {
            value
                .pointer("/status")
                .or_else(|| value.pointer("/error/status"))
        })
        .and_then(value_as_status)
        .or_else(|| diagnostic.and_then(status_from_text))
}

fn value_as_status(value: &Value) -> Option<u16> {
    value
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .or_else(|| value.as_str().and_then(status_from_text))
}

fn status_from_text(value: &str) -> Option<u16> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .filter(|token| !token.is_empty())
        .filter_map(|token| token.parse::<u16>().ok())
        .find(|status| {
            matches!(
                status,
                400 | 401 | 403 | 404 | 413 | 429 | 500 | 502 | 503 | 504
            )
        })
}

pub(super) fn extract_diagnostic(value: &Value) -> Option<String> {
    let message = ["/error/message", "/message", "/result", "/error"]
        .into_iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .map(str::to_owned);
    message.or_else(|| {
        let fields = ["/error/code", "/code", "/status", "/subtype", "/type"]
            .into_iter()
            .filter_map(|pointer| value.pointer(pointer))
            .filter(|field| field.is_string() || field.is_number())
            .map(Value::to_string)
            .collect::<Vec<_>>();
        (!fields.is_empty()).then(|| fields.join(" "))
    })
}

fn contains_any(value: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| value.contains(marker))
}
