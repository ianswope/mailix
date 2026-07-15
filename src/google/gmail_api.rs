//! A thin REST client over the Gmail API v1 — the mail analogue of calix's
//! `calendar_api.rs`. Everything blocks on network I/O; call from a background
//! thread. Gmail's native model (labels, threads, search) maps directly onto
//! these calls, which is exactly why Mailix speaks the Gmail API rather than
//! IMAP for Google accounts.

use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use base64::Engine;
use oauth2::reqwest;
use serde::Deserialize;
use url::Url;

const API_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

/// Gmail encodes part bodies as base64url, sometimes without padding — decode
/// leniently so either form works.
const B64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

// --- profile ---

#[derive(Deserialize)]
pub struct Profile {
    #[serde(rename = "emailAddress")]
    pub email_address: String,
    #[serde(rename = "historyId")]
    pub history_id: Option<String>,
}

/// The signed-in account's email + current history id (a stable account
/// identity without needing extra profile scopes).
pub fn get_profile(access_token: &str) -> Result<Profile, String> {
    let body = get(access_token, &format!("{API_BASE}/profile"))?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

// --- labels ---

#[derive(Deserialize)]
pub struct ApiLabel {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub label_type: Option<String>,
    pub color: Option<LabelColor>,
}

#[derive(Deserialize)]
pub struct LabelColor {
    #[serde(rename = "backgroundColor")]
    pub background_color: Option<String>,
}

impl ApiLabel {
    /// `"system"` (INBOX, SENT, …) or `"user"` (labels the user made).
    pub fn kind(&self) -> &str {
        if self.label_type.as_deref() == Some("system") {
            "system"
        } else {
            "user"
        }
    }

    pub fn color(&self) -> Option<String> {
        self.color
            .as_ref()
            .and_then(|c| c.background_color.clone())
    }
}

#[derive(Deserialize)]
struct LabelsResponse {
    #[serde(default)]
    labels: Vec<ApiLabel>,
}

pub fn list_labels(access_token: &str) -> Result<Vec<ApiLabel>, String> {
    let body = get(access_token, &format!("{API_BASE}/labels"))?;
    Ok(serde_json::from_str::<LabelsResponse>(&body)
        .map_err(|e| e.to_string())?
        .labels)
}

// --- threads ---

#[derive(Deserialize)]
pub struct ThreadRef {
    pub id: String,
    pub snippet: Option<String>,
    #[serde(rename = "historyId")]
    pub history_id: Option<String>,
}

#[derive(Deserialize)]
struct ThreadsResponse {
    #[serde(default)]
    threads: Vec<ThreadRef>,
    #[serde(rename = "nextPageToken")]
    #[allow(dead_code)]
    next_page_token: Option<String>,
}

/// The first page of thread ids matching a Gmail search `query` (e.g.
/// `"in:inbox"`), newest first, capped at `max`.
pub fn list_thread_ids(
    access_token: &str,
    query: &str,
    max: u32,
) -> Result<Vec<ThreadRef>, String> {
    let mut url = Url::parse(&format!("{API_BASE}/threads")).map_err(|e| e.to_string())?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("maxResults", &max.to_string());
    let body = get(access_token, url.as_str())?;
    Ok(serde_json::from_str::<ThreadsResponse>(&body)
        .map_err(|e| e.to_string())?
        .threads)
}

#[derive(Deserialize)]
pub struct ApiThread {
    pub id: String,
    #[serde(default)]
    pub messages: Vec<ApiMessage>,
}

#[derive(Deserialize)]
pub struct ApiMessage {
    pub id: String,
    #[serde(default, rename = "labelIds")]
    pub label_ids: Vec<String>,
    pub snippet: Option<String>,
    #[serde(rename = "internalDate")]
    pub internal_date: Option<String>,
    pub payload: Option<Payload>,
}

#[derive(Deserialize)]
pub struct Payload {
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
    pub filename: Option<String>,
    #[serde(default)]
    pub headers: Vec<Header>,
    pub body: Option<PartBody>,
    #[serde(default)]
    pub parts: Vec<Payload>,
}

#[derive(Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Deserialize)]
pub struct PartBody {
    pub data: Option<String>,
    #[serde(rename = "attachmentId")]
    pub attachment_id: Option<String>,
}

impl ApiMessage {
    /// A header value, case-insensitively (`"From"`, `"Subject"`, …).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.payload
            .as_ref()?
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }

    pub fn is_unread(&self) -> bool {
        self.label_ids.iter().any(|l| l == "UNREAD")
    }

    /// Gmail's `internalDate` is epoch milliseconds as a string.
    pub fn internal_date_ms(&self) -> Option<i64> {
        self.internal_date.as_ref()?.parse().ok()
    }
}

/// Thread with just enough per-message data (labels, internalDate, headers) to
/// build the conversation list — cheaper than pulling full bodies.
pub fn get_thread_metadata(access_token: &str, thread_id: &str) -> Result<ApiThread, String> {
    let mut url =
        Url::parse(&format!("{API_BASE}/threads/{thread_id}")).map_err(|e| e.to_string())?;
    url.query_pairs_mut()
        .append_pair("format", "metadata")
        .append_pair("metadataHeaders", "From")
        .append_pair("metadataHeaders", "To")
        .append_pair("metadataHeaders", "Cc")
        .append_pair("metadataHeaders", "Subject")
        .append_pair("metadataHeaders", "Date");
    let body = get(access_token, url.as_str())?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

/// Full thread including MIME part bodies, for rendering.
pub fn get_thread_full(access_token: &str, thread_id: &str) -> Result<ApiThread, String> {
    let mut url =
        Url::parse(&format!("{API_BASE}/threads/{thread_id}")).map_err(|e| e.to_string())?;
    url.query_pairs_mut().append_pair("format", "full");
    let body = get(access_token, url.as_str())?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

/// The decoded body of one message: the richest available representation.
pub struct ExtractedBody {
    pub text_plain: Option<String>,
    pub html: Option<String>,
}

/// Walks a message's MIME tree and pulls out the first text/plain and
/// text/html parts (base64url-decoded).
pub fn extract_body(message: &ApiMessage) -> ExtractedBody {
    let mut out = ExtractedBody {
        text_plain: None,
        html: None,
    };
    if let Some(payload) = &message.payload {
        walk_parts(payload, &mut out);
    }
    out
}

fn walk_parts(part: &Payload, out: &mut ExtractedBody) {
    match part.mime_type.as_deref() {
        Some("text/html") if out.html.is_none() => out.html = decode_part(part),
        Some("text/plain") if out.text_plain.is_none() => out.text_plain = decode_part(part),
        _ => {}
    }
    for child in &part.parts {
        walk_parts(child, out);
    }
}

fn decode_part(part: &Payload) -> Option<String> {
    let data = part.body.as_ref()?.data.as_ref()?;
    let bytes = B64.decode(data.as_bytes()).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

// --- send identities (sendAs) ---

#[derive(Deserialize)]
pub struct SendAs {
    #[serde(rename = "sendAsEmail")]
    pub email: String,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(rename = "isDefault", default)]
    pub is_default: bool,
    #[serde(rename = "isPrimary", default)]
    pub is_primary: bool,
}

#[derive(Deserialize)]
struct SendAsResponse {
    #[serde(default, rename = "sendAs")]
    send_as: Vec<SendAs>,
}

/// The account's send-as identities, including their native HTML signatures.
pub fn list_send_as(access_token: &str) -> Result<Vec<SendAs>, String> {
    let body = get(access_token, &format!("{API_BASE}/settings/sendAs"))?;
    Ok(serde_json::from_str::<SendAsResponse>(&body)
        .map_err(|e| e.to_string())?
        .send_as)
}

/// Sends a base64url-encoded RFC 822 message. `thread_id` threads a reply into
/// its conversation.
pub fn send_message(
    access_token: &str,
    raw_base64: &str,
    thread_id: Option<&str>,
) -> Result<(), String> {
    let mut body = serde_json::json!({ "raw": raw_base64 });
    if let Some(thread_id) = thread_id {
        body["threadId"] = serde_json::Value::String(thread_id.to_string());
    }
    post_json(access_token, &format!("{API_BASE}/messages/send"), &body)
}

/// Adds/removes labels on every message in a thread (Gmail's conversation-level
/// modify). Empty slices are fine. Used for read/unread, archive, and star.
pub fn modify_thread(
    access_token: &str,
    thread_id: &str,
    add: &[&str],
    remove: &[&str],
) -> Result<(), String> {
    let body = serde_json::json!({ "addLabelIds": add, "removeLabelIds": remove });
    post_json(access_token, &format!("{API_BASE}/threads/{thread_id}/modify"), &body)
}

/// Moves an entire thread to Trash.
pub fn trash_thread(access_token: &str, thread_id: &str) -> Result<(), String> {
    post_json(
        access_token,
        &format!("{API_BASE}/threads/{thread_id}/trash"),
        &serde_json::json!({}),
    )
}

fn get(access_token: &str, url: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::new();
    let response = client
        .get(url)
        .bearer_auth(access_token)
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("Gmail API error ({status}): {body}"));
    }
    Ok(body)
}

fn post_json(
    access_token: &str,
    url: &str,
    body: &serde_json::Value,
) -> Result<(), String> {
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(url)
        .bearer_auth(access_token)
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(body).map_err(|e| e.to_string())?)
        .send()
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("Gmail API error ({status}): {text}"));
    }
    Ok(())
}
