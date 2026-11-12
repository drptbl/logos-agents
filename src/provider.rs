//! Providers that run on their own: offers read from a file, and the HTTP
//! handler that does each paid job. The design is in `docs/providers.md`.

use std::collections::BTreeSet;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use logos_mcp::guard;

use crate::protocol::Payload;

/// The delivery window an offer promises when it names none, as `service_offer`'s default.
const DEFAULT_DELIVER_S: u64 = 600;
/// The longest delivery window an offer may promise, and an escrow quote may
/// lock a buyer's money for beyond its own life.
pub const MAX_DELIVER_S: u64 = 24 * 60 * 60;
/// The most content a job's input or output carries through storage.
pub const MAX_CONTENT_BYTES: usize = 8 * 1024 * 1024;
/// The most of a handler's answer read: content, base64 encoded, with room for
/// JSON that escapes its slashes.
const MAX_ANSWER_BYTES: u64 = (MAX_CONTENT_BYTES as u64).div_ceil(3) * 4 + 1024 * 1024;
/// The most of a quote handler's answer read: a price is small.
const MAX_QUOTE_BYTES: u64 = 1024 * 1024;
/// The most of a declining handler's reason passed on to the buyer.
const MAX_REASON_CHARS: usize = 200;

/// One offer, as an offers file names it.
#[derive(Debug, Clone, PartialEq)]
pub struct OfferSpec {
    pub service_id: String,
    pub tag: String,
    pub price_lepta: String,
    pub deliver_within_s: u64,
    pub escrow_account: Option<String>,
    /// Where paid jobs go. Without one, the offer waits for an agent.
    pub handler: Option<String>,
    /// Jobs taken at once, quoted or paid and not yet delivered.
    pub max_jobs: u32,
    /// Where requests are priced, if not at `price_lepta`. Only with a handler.
    pub quote_handler: Option<String>,
    /// Whether the handlers may be on another machine.
    pub remote_handler: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OffersFile {
    #[serde(default)]
    offer: Vec<RawOffer>,
}

/// An offer as a file or the app writes it, before its checks.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawOffer {
    service_id: String,
    tag: String,
    price_lepta: String,
    deliver_within_s: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    escrow_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    handler: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quote_handler: Option<String>,
    max_jobs: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    remote_handler: bool,
}

impl From<&OfferSpec> for RawOffer {
    fn from(o: &OfferSpec) -> Self {
        Self {
            service_id: o.service_id.clone(),
            tag: o.tag.clone(),
            price_lepta: o.price_lepta.clone(),
            deliver_within_s: Some(o.deliver_within_s),
            escrow_account: o.escrow_account.clone(),
            handler: o.handler.clone(),
            quote_handler: o.quote_handler.clone(),
            max_jobs: Some(o.max_jobs),
            remote_handler: o.remote_handler,
        }
    }
}

/// The offers in an offers file.
pub fn parse_offers(text: &str) -> Result<Vec<OfferSpec>, String> {
    let file: OffersFile = toml::from_str(text).map_err(|e| e.to_string())?;
    checked(file.offer)
}

/// The offers the Basecamp app sends, a JSON list, under the same checks as a file's.
pub fn offers_from_json(text: &str) -> Result<Vec<OfferSpec>, String> {
    let raw: Vec<RawOffer> = serde_json::from_str(text).map_err(|e| e.to_string())?;
    checked(raw)
}

/// The offers as the app shows them.
pub fn offers_to_json(offers: &[OfferSpec]) -> serde_json::Value {
    serde_json::to_value(offers.iter().map(RawOffer::from).collect::<Vec<_>>()).unwrap_or_default()
}

/// The offers as an offers file.
pub fn render_offers(offers: &[OfferSpec]) -> String {
    #[derive(Serialize)]
    struct File {
        offer: Vec<RawOffer>,
    }
    toml::to_string(&File {
        offer: offers.iter().map(RawOffer::from).collect(),
    })
    .unwrap_or_default()
}

/// One broken offer refuses them all, so a typo never quietly drops an offer.
fn checked(raw: Vec<RawOffer>) -> Result<Vec<OfferSpec>, String> {
    let mut seen = BTreeSet::new();
    raw.into_iter()
        .map(|raw| {
            if !seen.insert(raw.service_id.clone()) {
                return Err(format!("{} is offered twice", raw.service_id));
            }
            // The checks service_offer makes, here so a bad offer is refused at
            // once rather than retried.
            guard::validate_service_id(&raw.service_id)?;
            if raw.tag.len() > guard::MAX_AD_TAG_BYTES {
                return Err(format!(
                    "{}: tag is {} bytes, more than discovery's {}",
                    raw.service_id,
                    raw.tag.len(),
                    guard::MAX_AD_TAG_BYTES
                ));
            }
            if guard::amount_le16_hex(&raw.price_lepta).is_err()
                || raw.price_lepta.trim_start_matches('0').is_empty()
            {
                return Err(format!(
                    "{}: price_lepta must be a whole number of lepta above zero",
                    raw.service_id
                ));
            }
            if raw
                .deliver_within_s
                .is_some_and(|s| s == 0 || s > MAX_DELIVER_S)
            {
                return Err(format!(
                    "{}: deliver_within_s must be between 1 and {MAX_DELIVER_S}",
                    raw.service_id
                ));
            }
            if raw.max_jobs == Some(0) {
                return Err(format!("{}: max_jobs must be at least 1", raw.service_id));
            }
            if let Some(url) = &raw.handler {
                check_handler(url, raw.remote_handler)
                    .map_err(|e| format!("{}: {e}", raw.service_id))?;
            }
            if let Some(url) = &raw.quote_handler {
                if raw.handler.is_none() {
                    return Err(format!(
                        "{}: quote_handler prices jobs a handler does, so it needs a handler",
                        raw.service_id
                    ));
                }
                check_handler(url, raw.remote_handler)
                    .map_err(|e| format!("{}: quote_handler: {e}", raw.service_id))?;
            }
            Ok(OfferSpec {
                service_id: raw.service_id,
                tag: raw.tag,
                price_lepta: raw.price_lepta,
                deliver_within_s: raw.deliver_within_s.unwrap_or(DEFAULT_DELIVER_S),
                escrow_account: raw.escrow_account,
                handler: raw.handler,
                quote_handler: raw.quote_handler,
                max_jobs: raw.max_jobs.unwrap_or(1),
                remote_handler: raw.remote_handler,
            })
        })
        .collect()
}

/// The offers in the file at `path`.
pub fn load_offers(path: &std::path::Path) -> Result<Vec<OfferSpec>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    parse_offers(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// A handler is an http(s) URL on this machine, unless the offer allows
/// another: a typo must not send buyers' inputs elsewhere.
fn check_handler(url: &str, remote: bool) -> Result<(), String> {
    let uri: ureq::http::Uri = url
        .parse()
        .map_err(|_| format!("handler {url:?} is not a URL"))?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) {
        return Err(format!("handler {url:?} must be http or https"));
    }
    let host = uri
        .host()
        .ok_or_else(|| format!("handler {url:?} names no host"))?;
    let local = matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]");
    if !local && !remote {
        return Err(format!(
            "handler {url:?} isn't on this machine: set remote_handler = true to allow it"
        ));
    }
    Ok(())
}

/// What a handler is asked to do for one paid job.
#[derive(Debug, Serialize)]
pub struct HandlerRequest<'a> {
    pub job: &'a str,
    pub service_id: &'a str,
    /// As the buyer sent it, so untrusted.
    pub input: &'a HandlerInput,
    pub price_lepta: &'a str,
    pub deadline_ms: u64,
}

/// A job's input as a handler gets it: the buyer's text, or their CID with
/// its content when the server fetched it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HandlerInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_b64: Option<String>,
}

impl From<&Payload> for HandlerInput {
    fn from(input: &Payload) -> Self {
        match input {
            Payload::Text(t) => Self {
                text: Some(t.clone()),
                ..Self::default()
            },
            Payload::Cid(c) => Self {
                cid: Some(c.clone()),
                ..Self::default()
            },
        }
    }
}

impl HandlerInput {
    /// The same input, carrying `content` fetched for its CID.
    pub fn with_content(self, content: &[u8]) -> Self {
        Self {
            content_b64: Some(STANDARD.encode(content)),
            ..self
        }
    }
}

/// A handler's output: text, a CID, or content for the server to upload.
#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Text(String),
    Cid(String),
    Content(Vec<u8>),
}

/// A handler's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The job's output, to deliver.
    Output(Output),
    /// The handler can't do this job: decline it, giving this reason.
    Declined(String),
    /// It may work later: a server error, a timeout or no connection.
    Retry(String),
}

#[derive(Deserialize)]
struct Done {
    output: RawOutput,
}

#[derive(Deserialize)]
struct RawOutput {
    text: Option<String>,
    cid: Option<String>,
    content_b64: Option<String>,
}

impl RawOutput {
    /// Exactly one of its fields, with content decoded, and text or content
    /// within the cap.
    fn output(self) -> Result<Output, String> {
        let over = || {
            Err(format!(
                "the handler's output is over {} MiB",
                MAX_CONTENT_BYTES / (1024 * 1024)
            ))
        };
        match (self.text, self.cid, self.content_b64) {
            (Some(t), None, None) if t.len() > MAX_CONTENT_BYTES => over(),
            (Some(t), None, None) => Ok(Output::Text(t)),
            (None, Some(c), None) => Ok(Output::Cid(c)),
            (None, None, Some(b)) => {
                let content = STANDARD
                    .decode(b)
                    .map_err(|e| format!("the handler's content_b64 isn't base64: {e}"))?;
                if content.is_empty() {
                    return Err("the handler's output is empty".into());
                }
                if content.len() > MAX_CONTENT_BYTES {
                    return over();
                }
                Ok(Output::Content(content))
            }
            _ => {
                Err("the handler's output must have exactly one of text, cid or content_b64".into())
            }
        }
    }
}

/// The client for handler requests: no proxy and no redirects, so a request
/// checked to stay on this machine does, and an answer comes only from the
/// handler itself.
fn handler_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .proxy(None)
        .max_redirects(0)
        .build()
        .into()
}

/// What came back from a handler: its status and body, or why nothing did.
enum Exchange {
    /// The request couldn't be encoded: the same next time.
    Unsendable(String),
    Answered {
        status: ureq::http::StatusCode,
        /// The body, or why it couldn't be read: too big, or not text.
        body: Result<String, String>,
    },
    Failed(String),
}

/// POST `request` as JSON to `url`, waiting at most `timeout` and reading at
/// most `limit` bytes of the answer. Blocking.
fn exchange(url: &str, request: &impl Serialize, timeout: Duration, limit: u64) -> Exchange {
    let Ok(body) = serde_json::to_vec(request) else {
        return Exchange::Unsendable("the request couldn't be encoded".into());
    };
    let agent = handler_agent(timeout);
    let sent = agent
        .post(url)
        .header("content-type", "application/json")
        .send(&body[..]);
    let mut response = match sent {
        Ok(r) => r,
        Err(e) => return Exchange::Failed(e.to_string()),
    };
    let status = response.status();
    let body = response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_string()
        .map_err(|e| e.to_string());
    Exchange::Answered { status, body }
}

/// A declining handler's reason, as the buyer is shown it.
fn reason(body: Result<String, String>, status: ureq::http::StatusCode) -> String {
    let reason: String = body
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_REASON_CHARS)
        .collect();
    if reason.trim().is_empty() {
        format!("the handler declined the job ({status})")
    } else {
        reason.trim().to_string()
    }
}

/// Ask the handler at `url` to do a job, waiting at most `timeout`. Blocking.
pub fn call_handler(url: &str, job: &HandlerRequest<'_>, timeout: Duration) -> Answer {
    let (status, body) = match exchange(url, job, timeout, MAX_ANSWER_BYTES) {
        Exchange::Answered { status, body } => (status, body),
        Exchange::Unsendable(e) => return Answer::Declined(e),
        Exchange::Failed(e) => return Answer::Retry(e),
    };
    if status.is_success() {
        // An answer too big, or not text, would be the same next time.
        return match body.map(|b| serde_json::from_str::<Done>(&b)) {
            Ok(Ok(done)) => done
                .output
                .output()
                .map_or_else(Answer::Declined, Answer::Output),
            Ok(Err(e)) => Answer::Declined(format!("the handler's answer had no output: {e}")),
            Err(e) => Answer::Declined(format!("the handler's answer can't be read: {e}")),
        };
    }
    if status.is_redirection() {
        return Answer::Declined(format!("the handler answered with a redirect ({status})"));
    }
    if status.is_client_error() {
        return Answer::Declined(reason(body, status));
    }
    Answer::Retry(format!("the handler answered {status}"))
}

/// What a quote handler is asked to price: a request, before any quote.
#[derive(Debug, Serialize)]
pub struct QuoteRequest<'a> {
    pub job: &'a str,
    pub service_id: &'a str,
    /// As the buyer sent it, so untrusted.
    pub input: &'a Payload,
    /// The offer's own price, for a handler that adjusts it.
    pub price_lepta: &'a str,
}

/// A quote handler's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Priced {
    /// Quote this, and this delivery window if given.
    Price {
        price_lepta: String,
        deliver_within_s: Option<u64>,
    },
    /// Don't take this request: decline it, giving this reason.
    Declined(String),
    /// Ask again on a later pass.
    Retry(String),
}

#[derive(Deserialize)]
struct Price {
    price_lepta: String,
    deliver_within_s: Option<u64>,
}

/// Ask the quote handler at `url` to price a request, waiting at most `timeout`. Blocking.
pub fn call_quote(url: &str, request: &QuoteRequest<'_>, timeout: Duration) -> Priced {
    let (status, body) = match exchange(url, request, timeout, MAX_QUOTE_BYTES) {
        Exchange::Answered { status, body } => (status, body),
        Exchange::Unsendable(e) => return Priced::Declined(e),
        Exchange::Failed(e) => return Priced::Retry(e),
    };
    if status.is_success() {
        let price = body.and_then(|b| serde_json::from_str::<Price>(&b).map_err(|e| e.to_string()));
        // In its plain form: a padded price is one the protocol won't read.
        return match price {
            Ok(p)
                if p.deliver_within_s
                    .is_none_or(|s| (1..=MAX_DELIVER_S).contains(&s)) =>
            {
                match p.price_lepta.parse::<u128>() {
                    Ok(lepta) if lepta > 0 && p.price_lepta.bytes().all(|b| b.is_ascii_digit()) => {
                        Priced::Price {
                            price_lepta: lepta.to_string(),
                            deliver_within_s: p.deliver_within_s,
                        }
                    }
                    _ => Priced::Declined("the provider couldn't price this request".into()),
                }
            }
            Ok(_) => Priced::Declined("the provider couldn't price this request".into()),
            Err(e) => Priced::Declined(format!("the provider couldn't price this request: {e}")),
        };
    }
    if status.is_client_error() || status.is_redirection() {
        return Priced::Declined(reason(body, status));
    }
    Priced::Retry(format!("the quote handler answered {status}"))
}

#[cfg(test)]
mod tests;
