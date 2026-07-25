use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

pub(crate) const HEADER_NAME: &str = "x-claudex-working-directory";

pub(crate) fn custom_headers(
    existing: Option<&OsStr>,
    cwd: &Path,
    disabled_subagent_models: Option<&str>,
) -> String {
    let mut headers = existing
        .map(|value| value.to_string_lossy())
        .into_iter()
        .flat_map(|value| {
            value
                .lines()
                .filter(|line| !reserved_header(line))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    headers.push(format!("{HEADER_NAME}: {}", encode(cwd)));
    if let Some(models) = disabled_subagent_models {
        headers.push(format!("{}: {models}", crate::subagent_policy::HEADER_NAME));
    }
    headers.join("\n")
}

pub(crate) fn encode(path: &Path) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::new();
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

pub(crate) fn decode(value: &str) -> Option<PathBuf> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let high = hex_value(*bytes.get(index + 1)?)?;
        let low = hex_value(*bytes.get(index + 2)?)?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).ok().map(PathBuf::from)
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn reserved_header(line: &str) -> bool {
    line.split_once(':').is_some_and(|(name, _)| {
        let name = name.trim();
        name.eq_ignore_ascii_case(HEADER_NAME)
            || name.eq_ignore_ascii_case(crate::subagent_policy::HEADER_NAME)
    })
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn round_trips_visible_and_encoded_paths() {
        for path in ["/tmp/project", "/tmp/space and 日本語/%"] {
            assert_eq!(decode(&encode(Path::new(path))), Some(PathBuf::from(path)));
        }
    }

    #[test]
    fn merges_custom_headers_and_replaces_the_reserved_header() {
        let headers = custom_headers(
            Some(OsStr::new(
                "x-user-header: keep\nX-CLAUDEX-WORKING-DIRECTORY: forged\nX-CLAUDEX-DISABLED-SUBAGENT-MODELS: forged",
            )),
            Path::new("/tmp/project with space"),
            Some("gpt-5.6-sol,grok-4.5"),
        );
        assert_eq!(
            headers,
            "x-user-header: keep\nx-claudex-working-directory: /tmp/project%20with%20space\nx-claudex-disabled-subagent-models: gpt-5.6-sol,grok-4.5"
        );
        assert_eq!(
            custom_headers(None, Path::new("/tmp/plain"), None),
            "x-claudex-working-directory: /tmp/plain"
        );
    }

    #[test]
    fn rejects_malformed_percent_encoding_and_utf8() {
        for invalid in ["/tmp/%", "/tmp/%0", "/tmp/%GG", "/tmp/%FF"] {
            assert!(decode(invalid).is_none());
        }
    }
}
