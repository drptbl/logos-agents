use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use super::*;
use crate::protocol::Payload;

const OFFERS: &str = r#"
[[offer]]
service_id = "acme/summarize/v1"
tag = "summarize"
price_lepta = "1000000"
deliver_within_s = 3600
handler = "http://127.0.0.1:8080/summarize"
max_jobs = 2

[[offer]]
service_id = "acme/manual/v1"
tag = "by hand"
price_lepta = "5"
"#;

#[test]
fn offers_read_from_a_file_with_their_defaults() {
    let offers = parse_offers(OFFERS).unwrap();
    assert_eq!(offers.len(), 2);
    assert_eq!(offers[0].service_id, "acme/summarize/v1");
    assert_eq!(
        offers[0].handler.as_deref(),
        Some("http://127.0.0.1:8080/summarize")
    );
    assert_eq!(offers[0].max_jobs, 2);
    assert_eq!(offers[0].deliver_within_s, 3600);
    // Without a handler, an offer waits for an agent, as service_offer's do.
    assert_eq!(offers[1].handler, None);
    assert_eq!(offers[1].max_jobs, 1);
    assert_eq!(offers[1].deliver_within_s, 600);
}

#[test]
fn a_handler_elsewhere_needs_to_be_allowed() {
    let remote = |extra: &str| {
        format!(
            "[[offer]]\nservice_id = \"a/b/v1\"\ntag = \"t\"\nprice_lepta = \"1\"\nhandler = \"https://jobs.example.com/run\"\n{extra}"
        )
    };
    assert!(
        parse_offers(&remote(""))
            .unwrap_err()
            .contains("remote_handler")
    );
    assert!(parse_offers(&remote("remote_handler = true")).is_ok());
    for local in [
        "http://localhost:1/x",
        "http://127.0.0.1:1/x",
        "http://[::1]:1/x",
    ] {
        let text = format!(
            "[[offer]]\nservice_id = \"a/b/v1\"\ntag = \"t\"\nprice_lepta = \"1\"\nhandler = \"{local}\""
        );
        assert!(parse_offers(&text).is_ok(), "{local}");
    }
}

#[test]
fn a_broken_offers_file_is_refused_whole() {
    let one = |body: &str| {
        format!("[[offer]]\nservice_id = \"a/b/v1\"\ntag = \"t\"\nprice_lepta = \"1\"\n{body}")
    };
    for bad in [
        one("max_jobs = 0"),
        one("handler = \"ftp://127.0.0.1/x\""),
        one("handler = \"not a url\""),
        one("hander = \"http://127.0.0.1/x\""),
        format!("{}\n{}", one(""), one("")),
        "offer = 3".to_string(),
        one("deliver_within_s = 0"),
        one("deliver_within_s = 86401"),
        one("").replace("price_lepta = \"1\"", "price_lepta = \"0\""),
        one("").replace("price_lepta = \"1\"", "price_lepta = \"1.5\""),
        one("").replace("tag = \"t\"", &format!("tag = \"{}\"", "t".repeat(34))),
        one("").replace("a/b/v1", "not a service id"),
    ] {
        assert!(parse_offers(&bad).is_err(), "{bad}");
    }
}

/// A one-shot HTTP server answering `status` and `body`, handing back the
/// request it received.
fn stub(status: &'static str, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/run", listener.local_addr().unwrap());
    let served = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut seen = Vec::new();
        let mut buf = [0u8; 4096];
        // Read the headers, then as much body as Content-Length says.
        loop {
            let n = conn.read(&mut buf).unwrap();
            seen.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&seen).to_string();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if seen.len() >= end + 4 + length {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        conn.write_all(reply.as_bytes()).unwrap();
        String::from_utf8_lossy(&seen).to_string()
    });
    (url, served)
}

fn request() -> HandlerRequest<'static> {
    static INPUT: std::sync::LazyLock<Payload> =
        std::sync::LazyLock::new(|| Payload::Text("summarise this".into()));
    HandlerRequest {
        job: "j1",
        service_id: "acme/summarize/v1",
        input: &INPUT,
        price_lepta: "1000000",
        deadline_ms: 1_794_400_000_000,
    }
}

#[test]
fn a_handler_gets_the_job_and_returns_its_output() {
    let (url, served) = stub("200 OK", r#"{"output": {"text": "a summary"}}"#);
    let answer = call_handler(&url, &request(), Duration::from_secs(5));
    assert_eq!(answer, Answer::Output(Payload::Text("a summary".into())));
    let seen = served.join().unwrap();
    assert!(seen.starts_with("POST /run "), "{seen}");
    let body: serde_json::Value =
        serde_json::from_str(&seen[seen.find("\r\n\r\n").unwrap() + 4..]).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "job": "j1",
            "service_id": "acme/summarize/v1",
            "input": {"text": "summarise this"},
            "price_lepta": "1000000",
            "deadline_ms": 1_794_400_000_000u64,
        })
    );
}

#[test]
fn a_client_error_declines_and_a_server_error_retries() {
    let (url, _served) = stub("422 Unprocessable Entity", "that isn't a document");
    assert_eq!(
        call_handler(&url, &request(), Duration::from_secs(5)),
        Answer::Declined("that isn't a document".into())
    );
    let (url, _served) = stub("503 Service Unavailable", "busy");
    assert!(matches!(
        call_handler(&url, &request(), Duration::from_secs(5)),
        Answer::Retry(_)
    ));
    // Nothing listening: try again later.
    let free = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/run", free.local_addr().unwrap());
    drop(free);
    assert!(matches!(
        call_handler(&url, &request(), Duration::from_secs(5)),
        Answer::Retry(_)
    ));
}

#[test]
fn an_answer_without_an_output_declines() {
    let (url, _served) = stub("200 OK", r#"{"result": "a summary"}"#);
    assert!(matches!(
        call_handler(&url, &request(), Duration::from_secs(5)),
        Answer::Declined(_)
    ));
}

#[test]
fn offers_from_the_app_pass_the_same_checks_and_round_trip() {
    let json = r#"[
        {"service_id": "acme/summarize/v1", "tag": "summarize", "price_lepta": "1000000",
         "deliver_within_s": 3600, "handler": "http://127.0.0.1:8080/summarize", "max_jobs": 2},
        {"service_id": "acme/manual/v1", "tag": "by hand", "price_lepta": "5"}
    ]"#;
    let offers = offers_from_json(json).unwrap();
    assert_eq!(offers, parse_offers(OFFERS).unwrap());
    // Saved as an offers file, they read back the same.
    assert_eq!(parse_offers(&render_offers(&offers)).unwrap(), offers);
    // And shown to the app as they came.
    let shown = offers_to_json(&offers);
    assert_eq!(offers_from_json(&shown.to_string()).unwrap(), offers);
    // The app's offers meet the file's rules.
    let remote = r#"[{"service_id": "a/b/v1", "tag": "t", "price_lepta": "1",
                      "handler": "https://jobs.example.com/run"}]"#;
    assert!(
        offers_from_json(remote)
            .unwrap_err()
            .contains("remote_handler")
    );
}

#[test]
fn a_redirect_or_an_answer_too_big_declines() {
    let (url, _served) = stub("302 Found", "");
    assert!(matches!(
        call_handler(&url, &request(), Duration::from_secs(5)),
        Answer::Declined(_)
    ));
    static BIG: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            r#"{{"output": {{"text": "{}"}}}}"#,
            "x".repeat(2 * 1024 * 1024)
        )
    });
    let (url, _served) = stub("200 OK", &BIG);
    assert!(matches!(
        call_handler(&url, &request(), Duration::from_secs(5)),
        Answer::Declined(_)
    ));
}

#[test]
fn a_handler_on_this_machine_is_reached_without_a_proxy() {
    // A proxy set for the process must not carry buyers' inputs off the machine.
    let (url, _served) = stub("200 OK", r#"{"output": {"text": "a summary"}}"#);
    let agent = handler_agent(Duration::from_secs(5));
    assert!(agent.config().proxy().is_none());
    assert_eq!(agent.config().max_redirects(), 0);
    assert_eq!(
        call_handler(&url, &request(), Duration::from_secs(5)),
        Answer::Output(Payload::Text("a summary".into()))
    );
}

#[test]
fn a_quote_handler_needs_a_handler_and_stays_on_this_machine() {
    let one = |body: &str| {
        format!("[[offer]]\nservice_id = \"a/b/v1\"\ntag = \"t\"\nprice_lepta = \"1\"\n{body}")
    };
    let ok = parse_offers(&one(
        "handler = \"http://127.0.0.1:1/run\"\nquote_handler = \"http://127.0.0.1:1/quote\"",
    ))
    .unwrap();
    assert_eq!(
        ok[0].quote_handler.as_deref(),
        Some("http://127.0.0.1:1/quote")
    );
    assert!(parse_offers(&one("quote_handler = \"http://127.0.0.1:1/quote\"")).is_err());
    assert!(parse_offers(&one("handler = \"http://127.0.0.1:1/run\"\nquote_handler = \"https://pricing.example.com/q\"")).is_err());
}

fn quote_request() -> QuoteRequest<'static> {
    static INPUT: std::sync::LazyLock<Payload> =
        std::sync::LazyLock::new(|| Payload::Text("summarise this".into()));
    QuoteRequest {
        job: "j1",
        service_id: "acme/summarize/v1",
        input: &INPUT,
        price_lepta: "1000000",
    }
}

#[test]
fn a_quote_handler_prices_declines_or_waits() {
    let (url, served) = stub(
        "200 OK",
        r#"{"price_lepta": "2500000", "deliver_within_s": 120}"#,
    );
    assert_eq!(
        call_quote(&url, &quote_request(), Duration::from_secs(5)),
        Priced::Price {
            price_lepta: "2500000".into(),
            deliver_within_s: Some(120),
        }
    );
    let seen = served.join().unwrap();
    let body: serde_json::Value =
        serde_json::from_str(&seen[seen.find("\r\n\r\n").unwrap() + 4..]).unwrap();
    assert_eq!(body["input"], serde_json::json!({"text": "summarise this"}));
    assert_eq!(body["price_lepta"], "1000000");

    let (url, _served) = stub("422 Unprocessable Entity", "too long to summarise");
    assert_eq!(
        call_quote(&url, &quote_request(), Duration::from_secs(5)),
        Priced::Declined("too long to summarise".into())
    );
    let (url, _served) = stub("503 Service Unavailable", "");
    assert!(matches!(
        call_quote(&url, &quote_request(), Duration::from_secs(5)),
        Priced::Retry(_)
    ));
    // A price that isn't one declines, rather than asking the same thing again.
    for bad in [
        r#"{"price_lepta": "0"}"#,
        r#"{"price_lepta": "1.5"}"#,
        r#"{"price_lepta": "5", "deliver_within_s": 0}"#,
        r#"{"cost": 5}"#,
    ] {
        let (url, _served) = stub("200 OK", bad);
        assert!(
            matches!(
                call_quote(&url, &quote_request(), Duration::from_secs(5)),
                Priced::Declined(_)
            ),
            "{bad}"
        );
    }
}
