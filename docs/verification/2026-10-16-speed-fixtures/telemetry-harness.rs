#![allow(dead_code)]
#[path = "telemetry-before.rs"]
mod before;
#[path = "telemetry-candidate.rs"]
mod candidate;

use serde_json::json;
use std::hint::black_box;
use std::time::{Duration, Instant};

fn equivalence_cases() -> Vec<String> {
    let fragments = [
        "ordinary agent activity without credentials",
        "Agent α checked rust/src/main.rs — finished 作業 safely.",
        "-----BEGIN PRIVATE KEY-----\nprivate\n-----END PRIVATE KEY-----",
        "-----BEGIN RSA PRIVATE KEY-----\r\nprivate\r\n-----END RSA PRIVATE KEY-----",
        "-----BEGIN EC PRIVATE KEY-----private-----END EC PRIVATE KEY-----",
        "-----BEGIN PRIVATE KEY-----unterminated",
        "OPENAI_API_KEY='private'",
        "AWS_SECRET_ACCESS_KEY=private",
        "AWS_ACCESS_KEY_ID=private",
        "MY_TOKEN=private",
        "CLIENT_PASSWORD:\"private words\"",
        "MY_CREDENTIAL=private;MY_CREDENTIALS='private'",
        "TOKEN = 'private'",
        "ſecret=private",
        "API_KEY=private",
        "Bearer private",
        "bEaReR private+/=",
        "Basic dXNlcjpwYXNzd29yZA==",
        "basic private",
        "api-key: private",
        "api_key='private'",
        "access-token:private",
        "refresh_token=private",
        "authorization=private",
        "cookie=private",
        "password='private words'",
        "client-secret=private",
        "sk-abcdefghijklmnop",
        "ghp_abcdefghijklmnop",
        "gho_abcdefghijklmnop",
        "ghu_abcdefghijklmnop",
        "ghs_abcdefghijklmnop",
        "ghr_abcdefghijklmnop",
        "github_pat_abcdefghijklmnop",
        "xoxb-abcdefghijkl",
        "xoxa-abcdefghijkl",
        "xoxp-abcdefghijkl",
        "xoxr-abcdefghijkl",
        "xoxs-abcdefghijkl",
        "AKIAABCDEFGHIJKLMNOP",
        "sk-abcdefghijklmno ghp_abcdefghijklmno AKIAABCDEFGHIJKLMNO",
        "https://user:pass@example.test/path?topic=public",
        "https://example.test/PAIR/private",
        "http://example.test/pAiRiNg/private",
        "https://example.test/path?CoDe=private#private",
        "https://example.test/path?apiKEY=private",
        "https://example.test/path?monkey=public",
        "https://example.test/docs?topic=public",
        "https://example.test/repaired?topic=public",
        "http://example.test/作業?topic=public",
        "https://example.test/path#private",
        "https://one.test/docs https://two.test/path#private https://three.test/help",
        "OPENAI_API_KEY='-----BEGIN PRIVATE KEY-----\nprivate\n-----END PRIVATE KEY-----'",
        "authorization='Bearer private'",
        "api-key='https://user:pass@example.test/path?code=private'",
        "Bearer -----BEGIN PRIVATE KEY-----\nprivate\n-----END PRIVATE KEY-----",
        "token=sk-abcdefghijklmnop;password=private",
    ];
    let mut cases = vec![String::new(), "{}".into(), "[]".into(), "{invalid JSON".into()];
    let boundaries = ["", " ", "\n", "prefix_", "α", "🌳", "\"", "'"];
    for fragment in fragments {
        for left in boundaries {
            for right in boundaries {
                cases.push(format!("{left}{fragment}{right}"));
            }
        }
        cases.push(json!({"type":"response_item","payload":{"role":"assistant","content":[{"text":fragment}]}}).to_string());
        cases.push(json!({"arguments":json!({"output":fragment,"input_tokens":44}).to_string()}).to_string());
        cases.push(json!({fragment: "plain value", "safe": "plain value"}).to_string());
    }
    for first in fragments {
        for second in fragments {
            cases.push(format!("{first}{second}"));
            cases.push(format!("{first}\n{second}"));
        }
    }
    for key in ["env", "environment", "api_key", "API-KEY", "access_token", "refreshToken", "client-secret", "cookie", "authorization", "PASSWORD", "private key", "credentials", "next_token", "token", "input_tokens", "töken", "a\u{200b}pi_key", "API_KEY"] {
        cases.push(json!({key:"private-value", "safe":"text", "input_tokens":44}).to_string());
        cases.push(json!({key:44, "safe":"text", "input_tokens":44}).to_string());
    }
    let mut encoded = "OPENAI_API_KEY='private'".to_string();
    for _ in 0..10 {
        encoded = json!({"arguments":encoded}).to_string();
        cases.push(encoded.clone());
    }
    let mut nested = json!({"password":"private"});
    for _ in 0..40 {
        nested = json!({"safe":nested});
        cases.push(nested.to_string());
    }
    cases
}

fn verify() -> usize {
    let cases = equivalence_cases();
    for (index, input) in cases.iter().enumerate() {
        assert_eq!(before::redact(input), candidate::redact(input), "equivalence fixture {index}");
    }
    cases.len()
}

fn measure(f: fn(&str) -> String, inputs: &[String], rounds: usize) -> Duration {
    let start = Instant::now();
    for _ in 0..rounds {
        for input in inputs {
            black_box(f(black_box(input)));
        }
    }
    start.elapsed()
}

fn main() {
    println!("Equivalent fixtures: {}", verify());
    let safe = "Agent α read rust/src/main.rs, inspected the graph, and completed the requested work. View https://example.test/docs?topic=graphs for details.\n".repeat(32);
    let sensitive = format!("{safe}OPENAI_API_KEY='private' Bearer private https://user:pass@example.test/pair?code=private#fragment\n{safe}");
    let json_safe = json!({"type":"response_item","timestamp":"2026-10-16T12:00:00Z","payload":{"role":"assistant","content":[{"type":"output_text","text":safe}]}}).to_string();
    let json_sensitive = json!({"type":"response_item","payload":{"role":"tool","content":sensitive,"api_key":"private","input_tokens":44}}).to_string();
    let short = vec!["type".into(), "payload".into(), "input_tokens".into(), "2026-10-16T12:00:00Z".into(), "11111111-2222-3333-4444-555555555555".into()];
    let metadata = json!({"type":"response_item","payload":{"events":(0..40).map(|i| json!({"id":format!("call_{i:04}"),"kind":"tool","status":"completed","content":"Read rust/src/main.rs","timestamp":"2026-10-16T12:00:00Z"})).collect::<Vec<_>>()}}).to_string();
    println!("Metadata JSON bytes: {}",metadata.len());
    let short_sensitive = vec!["Bearer private".into(),"OPENAI_API_KEY='private'".into(),"sk-abcdefghijklmnop".into(),"https://u:p@a.test/pair?code=private".into()];
    let groups = [
        ("safe prose 4 KiB", vec![safe.clone()], 200),
        ("credential prose 9 KiB", vec![sensitive.clone()], 200),
        ("safe nested JSON 4 KiB", vec![json_safe.clone()], 200),
        ("credential nested JSON 9 KiB", vec![json_sensitive.clone()], 200),
        ("small telemetry keys and IDs", short, 1000),
        ("small credential-bearing fields", short_sensitive, 500),
        ("metadata-heavy JSON 5 KiB", vec![metadata], 200),
        ("mixed 90% safe prose", (0..10).map(|i| if i == 9 { sensitive.clone() } else { safe.clone() }).collect(), 50),
    ];
    for (name, inputs, rounds) in groups {
        for input in &inputs {
            assert_eq!(before::redact(input), candidate::redact(input));
        }
        measure(before::redact, &inputs, 10);
        measure(candidate::redact, &inputs, 10);
        let mut old = Vec::new();
        let mut new = Vec::new();
        for trial in 0..7 {
            if trial % 2 == 0 {
                old.push(measure(before::redact, &inputs, rounds));
                new.push(measure(candidate::redact, &inputs, rounds));
            } else {
                new.push(measure(candidate::redact, &inputs, rounds));
                old.push(measure(before::redact, &inputs, rounds));
            }
        }
        old.sort();
        new.sort();
        println!("{name}: {} calls, before={:.3}ms candidate={:.3}ms speedup={:.3}x", rounds*inputs.len(), old[3].as_secs_f64()*1000.0, new[3].as_secs_f64()*1000.0, old[3].as_secs_f64()/new[3].as_secs_f64());
    }
}

#[test]
fn equivalent_to_actual_previous_source() {
    assert!(verify() > 10_000);
}
