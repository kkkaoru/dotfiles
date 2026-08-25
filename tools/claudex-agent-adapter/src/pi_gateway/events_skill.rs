use serde_json::{Value, json};

pub(super) fn explicit_skill_arguments(request: &Value) -> Option<Value> {
    let user = latest_user_text(request)?;
    let system = request.get("system").map(value_text).unwrap_or_default();
    let tagged = tagged_values(&system, "name");
    let skill = explicit_field(&user, "skill")
        .or_else(|| named_skill(&user))
        .or_else(|| uniquely_mentioned_skill(&user, &tagged))?;
    let args = explicit_field(&user, "args").or_else(|| nearby_flag(&user, &skill));
    let mut recovered = json!({"skill":skill});
    if let Some(args) = args {
        recovered["args"] = json!(args);
    }
    Some(recovered)
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(values) => values.iter().map(value_text).collect::<Vec<_>>().join("\n"),
        Value::Object(object) => object
            .get("text")
            .or_else(|| object.get("content"))
            .map(value_text)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn latest_user_text(request: &Value) -> Option<String> {
    request
        .get("messages")?
        .as_array()?
        .iter()
        .rev()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .map(|message| user_instruction_text(message.get("content").unwrap_or(&Value::Null)))
        .find(|text| !text.trim().is_empty())
}

fn user_instruction_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn named_skill(text: &str) -> Option<String> {
    ["スキル `", "Skill `", "skill `"]
        .iter()
        .find_map(|marker| {
            let rest = text.split_once(marker)?.1;
            let value = rest.split('`').next()?.trim();
            valid_skill_name(value).then(|| value.to_owned())
        })
}

fn uniquely_mentioned_skill(text: &str, skills: &[String]) -> Option<String> {
    let mut mentioned = skills.iter().filter(|skill| text.contains(skill.as_str()));
    let skill = mentioned.next()?.clone();
    mentioned.next().is_none().then_some(skill)
}

fn valid_skill_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

fn tagged_values(text: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut values = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        let Some(end) = rest.find(&close) else {
            break;
        };
        let value = rest[..end].trim();
        if !value.is_empty() {
            values.push(value.to_owned());
        }
        rest = &rest[end + close.len()..];
    }
    values
}

fn explicit_field(text: &str, field: &str) -> Option<String> {
    let start = text.find(&format!("{field}="))? + field.len() + 1;
    let rest = text[start..].trim_start();
    let quote = rest
        .chars()
        .next()
        .filter(|ch| matches!(ch, '\"' | '\'' | '`'));
    let value = quote.map_or(rest, |ch| &rest[ch.len_utf8()..]);
    let end = quote.and_then(|ch| value.find(ch)).unwrap_or_else(|| {
        if value.starts_with("--") {
            value
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
                .unwrap_or(value.len())
        } else {
            value
                .find(|ch: char| ch.is_whitespace() || ch == ',')
                .unwrap_or(value.len())
        }
    });
    let result = value[..end].trim();
    (!result.is_empty()).then(|| result.to_owned())
}

fn nearby_flag(text: &str, skill: &str) -> Option<String> {
    let after_skill = &text[text.find(skill)? + skill.len()..];
    let nearby = &after_skill[..after_skill.len().min(160)];
    let start = nearby.find("--")?;
    let flag = nearby[start..]
        .trim_start_matches(['`', '\"', '\''])
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-'))
        .next()?;
    (flag.len() > 2).then(|| flag.to_owned())
}
