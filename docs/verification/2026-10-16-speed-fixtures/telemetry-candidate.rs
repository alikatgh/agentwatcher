use regex::Regex;
use serde_json::Value;
use std::borrow::Cow;
use std::sync::OnceLock;
/// Redacts credentials in free text, nested JSON strings, environment objects,
/// authorization headers and private URLs. Shared by telemetry and UI logs.
pub fn redact(text: &str) -> String {
    redact_text(text, 0)
}

fn redact_text(text: &str, depth: usize) -> String {
    if depth >= 8 && (text.trim_start().starts_with('{') || text.trim_start().starts_with('[')) {
        return "[nested JSON omitted]".into();
    }
    if depth < 8 && (text.trim_start().starts_with('{') || text.trim_start().starts_with('[')) {
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            return serde_json::to_string(&redact_value(&value, depth + 1))
                .unwrap_or_else(|_| "[unreadable record]".into());
        }
    }
    redact_free_text(text).into_owned()
}

fn redaction_rules() -> &'static [(Regex, &'static str)] {
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    RULES.get_or_init(|| [
        (r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----", "[private key redacted]"),
        (r#"(?i)\b([A-Z_][A-Z0-9_]*(?:API_KEY|SECRET_ACCESS_KEY|ACCESS_KEY_ID|TOKEN|PASSWORD|SECRET|PRIVATE_KEY|CREDENTIALS?)|TOKEN|SECRET|PASSWORD|API_KEY|ACCESS_KEY_ID)["']?\s*[:=]\s*(?:"[^"]*"|'[^']*'|[^\s"'`,;]+)"#, "$1=[redacted]"),
        (r"(?i)\b(Bearer|Basic)\s+[A-Za-z0-9._~+/=-]+", "$1 [redacted]"),
        (r#"(?i)\b(api[_-]?key|access[_-]?token|refresh[_-]?token|password|client[_-]?secret|authorization|cookie)["']?\s*[:=]\s*(?:"[^"]*"|'[^']*'|[^\s"'`,;]+)"#, "$1=[redacted]"),
        (r"\b(?:sk-[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{16,}|xox[baprs]-[A-Za-z0-9-]{12,}|AKIA[A-Z0-9]{16})\b", "[credential redacted]"),
    ].into_iter().map(|(pattern, replacement)| (Regex::new(pattern).expect("static redaction regex"), replacement)).collect())
}

fn redaction_urls() -> &'static Regex {
    static URLS: OnceLock<Regex> = OnceLock::new();
    URLS.get_or_init(|| Regex::new(r#"https?://[^\s<>"']+"#).expect("static URL regex"))
}

fn redact_free_text(text: &str) -> Cow<'_, str> {
    let mut safe = Cow::Borrowed(text);
    for (regex, replacement) in redaction_rules() {
        // Short metadata fields benefit from avoiding capture setup on misses.
        // The extra search costs more on larger payloads, so keep their path.
        if safe.len() <= 64 && !regex.is_match(safe.as_ref()) {
            continue;
        }
        if let Cow::Owned(replaced) = regex.replace_all(safe.as_ref(), *replacement) {
            safe = Cow::Owned(replaced);
        }
    }
    let mut output = None;
    let mut copied = 0;
    for matched in redaction_urls().find_iter(&safe) {
        let url = matched.as_str();
        let (scheme, rest) = url.split_once("://").unwrap_or(("https", ""));
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        let tail = &rest[authority_end..];
        let sensitive_query = tail.split_once('?').is_some_and(|(_, query)| {
            query.split('&').any(|pair| {
                let key = pair.split('=').next().unwrap_or("");
                [
                    "token",
                    "key",
                    "secret",
                    "password",
                    "auth",
                    "credential",
                    "code",
                ]
                .iter()
                .any(|part| contains_ascii_case_insensitive(key, part))
            })
        });
        if authority.contains('@')
            || tail.contains('#')
            || contains_ascii_case_insensitive(tail, "/pair")
            || sensitive_query
        {
            let output = output.get_or_insert_with(|| String::with_capacity(safe.len()));
            output.push_str(&safe[copied..matched.start()]);
            output.push_str(scheme);
            output.push_str("://");
            output.push_str(authority.rsplit('@').next().unwrap_or("[redacted]"));
            copied = matched.end();
        }
    }
    if let Some(mut output) = output {
        output.push_str(&safe[copied..]);
        Cow::Owned(output)
    } else {
        safe
    }
}

fn contains_ascii_case_insensitive(text: &str, pattern: &str) -> bool {
    text.as_bytes()
        .windows(pattern.len())
        .any(|window| window.eq_ignore_ascii_case(pattern.as_bytes()))
}

fn redact_value(value: &Value, depth: usize) -> Value {
    if depth > 32 {
        return Value::String("[nested value omitted]".into());
    }
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let normalized = key
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .flat_map(char::to_lowercase)
                        .collect::<String>();
                    let sensitive = [
                        "apikey",
                        "accesskey",
                        "accesstoken",
                        "refreshtoken",
                        "authorization",
                        "password",
                        "secret",
                        "credential",
                        "privatekey",
                        "clientsecret",
                        "cookie",
                    ]
                    .iter()
                    .any(|word| normalized.contains(word))
                        || (normalized.ends_with("token") && value.is_string())
                        || ["env", "environment"].contains(&normalized.as_str());
                    (
                        redact_text(key, depth + 1),
                        if sensitive {
                            Value::String("[redacted]".into())
                        } else {
                            redact_value(value, depth + 1)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(array) => Value::Array(
            array
                .iter()
                .map(|value| redact_value(value, depth + 1))
                .collect(),
        ),
        Value::String(text) => Value::String(redact_text(text, depth + 1)),
        _ => value.clone(),
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(&redact_value(value, 0))
        .unwrap_or_else(|_| "[unreadable record]".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Instant;
    #[test]
    fn redaction_is_consistent_for_env_nested_json_and_private_urls() {
        let input = json!({"arguments":json!({"env":{"SAFE_NAME":"also-hidden"},"api_key":"nested-secret","input_tokens":44}).to_string(),
            "output":"OPENAI_API_KEY='plain-secret' Bearer bearer-secret https://user:pass@example.test/pair?code=secret#fragment",
            "password":"object-secret"});
        let safe = redact(&input.to_string());
        for secret in [
            "also-hidden",
            "nested-secret",
            "plain-secret",
            "bearer-secret",
            "object-secret",
            "user:pass",
            "code=secret",
            "#fragment",
        ] {
            assert!(!safe.contains(secret), "leaked {secret}");
        }
        assert!(safe.contains("input_tokens"));
        assert!(safe.contains("44"));
        assert_eq!(redact(&safe), safe);
        assert!(
            !redact("-----BEGIN PRIVATE KEY-----\nsecret\n-----END PRIVATE KEY-----")
                .contains("secret")
        );
    }

    #[test]
    fn redaction_keeps_safe_text_borrowed_and_preserves_public_urls() {
        for input in [
            "Agent α read rust/src/main.rs and finished successfully.",
            "View https://example.test/docs?topic=graphs and http://example.test/作業 next.",
            "https://example.test/repaired?topic=public",
            "",
        ] {
            assert!(matches!(redact_free_text(input), Cow::Borrowed(_)));
            assert_eq!(redact(input), input);
        }
        assert_eq!(
            redact(
                "safe https://example.test/docs?topic=graphs private https://user:pass@example.test/path middle http://other.test/PAIR/abc public https://last.test/help end"
            ),
            "safe https://example.test/docs?topic=graphs private https://example.test middle http://other.test public https://last.test/help end"
        );
        for input in [
            "https://example.test/path?CoDe=private",
            "https://example.test/path?apiKEY=private",
            "https://example.test/path#private",
            "https://example.test/pAiRiNg/private",
        ] {
            assert_eq!(redact(input), "https://example.test");
        }
    }

    // Retain the previous allocation path for a deterministic, same-process
    // comparison. It shares compiled rules, so regex initialization is excluded.
    fn legacy_redact_free_text(text: &str) -> String {
        let mut safe = text.to_owned();
        for (regex, replacement) in redaction_rules() {
            safe = regex.replace_all(&safe, *replacement).into_owned();
        }
        redaction_urls()
            .replace_all(&safe, |capture: &regex::Captures<'_>| {
                let url = &capture[0];
                let (scheme, rest) = url.split_once("://").unwrap_or(("https", ""));
                let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
                let authority = &rest[..authority_end];
                let tail = &rest[authority_end..];
                let sensitive_query = tail.split_once('?').is_some_and(|(_, query)| {
                    query.split('&').any(|pair| {
                        let key = pair.split('=').next().unwrap_or("").to_ascii_lowercase();
                        [
                            "token",
                            "key",
                            "secret",
                            "password",
                            "auth",
                            "credential",
                            "code",
                        ]
                        .iter()
                        .any(|part| key.contains(part))
                    })
                });
                if authority.contains('@')
                    || tail.contains('#')
                    || tail.to_ascii_lowercase().contains("/pair")
                    || sensitive_query
                {
                    format!(
                        "{scheme}://{}",
                        authority.rsplit('@').next().unwrap_or("[redacted]")
                    )
                } else {
                    url.to_owned()
                }
            })
            .into_owned()
    }

    #[test]
    fn redaction_matches_previous_rules_across_safe_and_sensitive_text() {
        let cases = [
            "ordinary agent activity without credentials",
            "-----BEGIN RSA PRIVATE KEY-----\nprivate\n-----END RSA PRIVATE KEY-----",
            "OPENAI_API_KEY='private' AWS_SECRET_ACCESS_KEY=private PASSWORD=private",
            "Bearer private Basic dXNlcjpwYXNzd29yZA==",
            "api-key: private access_token=private refresh_token=private cookie=private",
            "sk-abcdefghijklmnop ghp_abcdefghijklmnop github_pat_abcdefghijklmnop xoxb-abcdefghijkl AKIAABCDEFGHIJKLMNOP",
            "https://user:pass@example.test/path?topic=public https://example.test/PAIR/private https://example.test/path?CoDe=private#private",
            "http://example.test/作業?topic=public https://example.test/path?monkey=public",
            "https://one.test/docs https://two.test/path#private https://three.test/help https://four.test/path?auth=private https://five.test/docs",
        ];
        for input in cases {
            assert_eq!(redact(input), legacy_redact_free_text(input), "{input}");
        }
    }

    #[test]
    fn redaction_short_fields_preserve_credentials_at_filter_boundaries() {
        for input in [
            "ordinary agent α activity",
            "OPENAI_API_KEY='private'",
            "API_KEY=private",
            "ſecret=private",
            "Baſic private",
            "Bearer private",
            "sk-abcdefghijklmnop",
            "ghp_abcdefghijklmnop",
            "https://u:p@example.test/pair?code=private",
            "-----BEGIN PRIVATE KEY-----\nprivate\n-----END PRIVATE KEY----- Bearer private",
        ] {
            for length in [0_usize, 63, 64, 65, 128] {
                let padded = format!("{input}{}", " ".repeat(length.saturating_sub(input.len())));
                assert_eq!(
                    redact(&padded),
                    legacy_redact_free_text(&padded),
                    "redaction changed for a {}-byte field",
                    padded.len()
                );
            }
        }
    }

    #[test]
    #[ignore = "manual same-process redaction performance comparison"]
    fn benchmark_redaction_allocation_path() {
        let safe = "Agent α read rust/src/main.rs, inspected the graph, and completed the requested work. View https://example.test/docs?topic=graphs for details.\n".repeat(32);
        let sensitive = format!(
            "{safe}OPENAI_API_KEY='private' Bearer private https://user:pass@example.test/pair?code=private#fragment\n{safe}"
        );
        let cases = [safe.as_str(), sensitive.as_str()];
        for input in cases {
            assert_eq!(redact(input), legacy_redact_free_text(input));
        }
        let measure = |redact: fn(&str) -> String| {
            let start = Instant::now();
            for _ in 0..500 {
                for input in cases {
                    std::hint::black_box(redact(std::hint::black_box(input)));
                }
            }
            start.elapsed()
        };
        let mut legacy = Vec::new();
        let mut current = Vec::new();
        for pass in 0..3 {
            if pass % 2 == 0 {
                legacy.push(measure(legacy_redact_free_text));
                current.push(measure(redact));
            } else {
                current.push(measure(redact));
                legacy.push(measure(legacy_redact_free_text));
            }
        }
        legacy.sort();
        current.sort();
        eprintln!(
            "Redaction (1,000 mixed 4–9 KiB records, median of 3): previous {:.2} ms, current {:.2} ms, {:.2}x",
            legacy[1].as_secs_f64() * 1_000.0,
            current[1].as_secs_f64() * 1_000.0,
            legacy[1].as_secs_f64() / current[1].as_secs_f64()
        );
    }

}
