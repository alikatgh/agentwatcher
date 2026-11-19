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
