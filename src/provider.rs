//! Providers that run on their own: offers read from a file, and the HTTP
//! handler that does each paid job. The design is in `docs/providers.md`.

use std::collections::BTreeSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::protocol::Payload;

/// The delivery window an offer promises when it names none, as `service_offer`'s default.
const DEFAULT_DELIVER_S: u64 = 600;
/// The most of a handler's answer read: an output must fit a chat message anyway.
const MAX_ANSWER_BYTES: u64 = 1024 * 1024;
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
    /// Whether the handler may be on another machine.
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
            if raw.max_jobs == Some(0) {
                return Err(format!("{}: max_jobs must be at least 1", raw.service_id));
            }
            if let Some(url) = &raw.handler {
                check_handler(url, raw.remote_handler)
                    .map_err(|e| format!("{}: {e}", raw.service_id))?;
            }
            Ok(OfferSpec {
                service_id: raw.service_id,
                tag: raw.tag,
                price_lepta: raw.price_lepta,
                deliver_within_s: raw.deliver_within_s.unwrap_or(DEFAULT_DELIVER_S),
                escrow_account: raw.escrow_account,
                handler: raw.handler,
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
    pub input: &'a Payload,
    pub price_lepta: &'a str,
    pub deadline_ms: u64,
}

/// A handler's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The job's output, to deliver.
    Output(Payload),
    /// The handler can't do this job: decline it, giving this reason.
    Declined(String),
    /// It may work later: a server error, a timeout or no connection.
    Retry(String),
}

#[derive(Deserialize)]
struct Done {
    output: Payload,
}

/// Ask the handler at `url` to do a job, waiting at most `timeout`. Blocking.
pub fn call_handler(url: &str, job: &HandlerRequest<'_>, timeout: Duration) -> Answer {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .into();
    let Ok(body) = serde_json::to_vec(job) else {
        return Answer::Declined("the job couldn't be encoded for the handler".into());
    };
    let request = agent.post(url).header("content-type", "application/json");
    let mut response = match request.send(&body[..]) {
        Ok(r) => r,
        Err(e) => return Answer::Retry(e.to_string()),
    };
    let status = response.status();
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_ANSWER_BYTES)
        .read_to_string();
    if status.is_success() {
        return match body.map(|b| serde_json::from_str::<Done>(&b)) {
            Ok(Ok(done)) => Answer::Output(done.output),
            Ok(Err(e)) => Answer::Declined(format!("the handler's answer had no output: {e}")),
            Err(e) => Answer::Retry(format!("reading the handler's answer failed: {e}")),
        };
    }
    if status.is_client_error() {
        let reason: String = body
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_REASON_CHARS)
            .collect();
        let reason = if reason.trim().is_empty() {
            format!("the handler declined the job ({status})")
        } else {
            reason.trim().to_string()
        };
        return Answer::Declined(reason);
    }
    Answer::Retry(format!("the handler answered {status}"))
}

#[cfg(test)]
mod tests;
