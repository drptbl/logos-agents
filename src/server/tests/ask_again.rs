//! A buyer's request that gets no answer: the follow-through sends it again,
//! unchanged, and cancels the job after three tries.

use super::*;

const MINUTE: u64 = 60_000;

/// A buyer's conversation holding its request, sent `asks` times three
/// minutes apart, the last `ago` ms before now, and then `later`.
fn asked(asks: usize, ago: u64, later: &[(bool, &str)]) -> RawOutput {
    let last = now_ms() - ago;
    let request = request_text();
    let mut list: Vec<Value> = (0..asks)
        .map(|i| {
            let at = last - (asks - 1 - i) as u64 * 3 * MINUTE;
            json!({ "content": request, "from_self": true, "timestamp_ms": at })
        })
        .collect();
    for (mine, text) in later {
        let mut m = json!({ "content": text, "from_self": mine, "timestamp_ms": last + 1 });
        if !mine {
            m["sender"] = json!(PEER);
        }
        list.push(m);
    }
    chat("get_messages", &Value::Array(list).to_string())
}

async fn buyer_pass(history: RawOutput) -> (Vec<String>, Arc<Routed>) {
    let fake = node();
    fake.set("chat_module.get_messages", history);
    let (s, _dir) = services_server(fake.clone());
    let done = s.follow_through().await;
    (done, fake)
}

#[tokio::test]
async fn a_request_with_no_answer_is_sent_again_unchanged() {
    let (done, fake) = buyer_pass(asked(1, 2 * MINUTE, &[])).await;
    assert_eq!(done, [format!("service_request {JOB}")]);
    let sends = fake.calls_to("chat_module.send_message");
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0][0], format!("str:{CONVO}"));
    assert_eq!(sends[0][1], format!("str:{}", request_text()));
}

#[tokio::test]
async fn a_request_is_not_sent_again_too_soon() {
    let (done, fake) = buyer_pass(asked(1, 20_000, &[])).await;
    assert!(done.is_empty(), "{done:?}");
    assert!(fake.calls_to("chat_module.send_message").is_empty());
}

#[tokio::test]
async fn an_answered_request_is_not_sent_again() {
    let (done, fake) = buyer_pass(asked(
        1,
        10 * MINUTE,
        &[(false, &quote_text("logos.dev", FAR))],
    ))
    .await;
    assert!(done.is_empty(), "{done:?}");
    assert!(fake.calls_to("chat_module.send_message").is_empty());
}

#[tokio::test]
async fn the_second_try_waits_longer() {
    let (done, _) = buyer_pass(asked(2, 2 * MINUTE, &[])).await;
    assert!(done.is_empty(), "{done:?}");
    let (done, fake) = buyer_pass(asked(2, 6 * MINUTE, &[])).await;
    assert_eq!(done, [format!("service_request {JOB}")]);
    assert_eq!(
        fake.calls_to("chat_module.send_message")[0][1],
        format!("str:{}", request_text())
    );
}

#[tokio::test]
async fn after_three_tries_with_no_answer_the_job_is_canceled() {
    let (done, _) = buyer_pass(asked(3, 6 * MINUTE, &[])).await;
    assert!(done.is_empty(), "{done:?}");
    let (done, fake) = buyer_pass(asked(3, 11 * MINUTE, &[])).await;
    assert_eq!(done, [format!("service_cancel {JOB}")]);
    let told = sent(&fake.calls_to("chat_module.send_message")[0]);
    assert!(
        matches!(&told.body, Body::Cancel { reason } if reason.contains("answer")),
        "{told:?}"
    );
}

#[tokio::test]
async fn a_providers_request_is_never_sent_again() {
    // The same request, received: it's the provider's to answer.
    let fake = node();
    fake.set(
        "chat_module.get_messages",
        chat(
            "get_messages",
            &json!([{ "content": request_text(), "from_self": false, "sender": PEER,
                      "timestamp_ms": now_ms() - 20 * MINUTE }])
            .to_string(),
        ),
    );
    let (s, _dir) = services_server(fake.clone());
    assert!(s.follow_through().await.is_empty());
    assert!(fake.calls_to("chat_module.send_message").is_empty());
}

#[tokio::test]
async fn a_quote_that_came_during_the_pass_is_not_canceled() {
    let fake = node();
    // The pass reads the request unanswered, then the quote lands.
    fake.set("chat_module.get_messages", asked(3, 11 * MINUTE, &[]));
    fake.set(
        "chat_module.get_messages",
        asked(3, 11 * MINUTE, &[(false, &quote_text("logos.dev", FAR))]),
    );
    let (s, _dir) = services_server(fake.clone());
    assert!(s.follow_through().await.is_empty());
    assert!(fake.calls_to("chat_module.send_message").is_empty());
}

#[tokio::test]
async fn a_request_long_past_every_try_is_canceled_without_asking_again() {
    let (done, fake) = buyer_pass(asked(1, 2 * 60 * MINUTE, &[])).await;
    assert_eq!(done, [format!("service_cancel {JOB}")]);
    let sends = fake.calls_to("chat_module.send_message");
    assert_eq!(sends.len(), 1);
    assert!(matches!(sent(&sends[0]).body, Body::Cancel { .. }));
}

#[tokio::test]
async fn a_request_without_times_is_left_alone() {
    let history = chat(
        "get_messages",
        &json!([{ "content": request_text(), "from_self": true }]).to_string(),
    );
    let (done, fake) = buyer_pass(history).await;
    assert!(done.is_empty(), "{done:?}");
    assert!(fake.calls_to("chat_module.send_message").is_empty());
}
