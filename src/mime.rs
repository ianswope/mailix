//! Builds an outgoing RFC 5322 message for Gmail's `messages.send`.
//!
//! `mail-builder` assembles the MIME tree (multipart/alternative for the
//! text+HTML bodies, wrapped in multipart/mixed when there are attachments);
//! we then base64url-encode it for the API's `raw` field. The plain-text
//! alternative is derived from the composed HTML so every message ships both.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE;
use mail_builder::MessageBuilder;

#[derive(Clone)]
pub struct Attachment {
    pub filename: String,
    pub mime: String,
    pub data: Vec<u8>,
}

pub struct OutgoingMessage {
    pub from_name: String,
    pub from_email: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub html_body: String,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub attachments: Vec<Attachment>,
}

/// Builds the message and returns it base64url-encoded, ready for the Gmail
/// `messages.send` `raw` field.
pub fn build_raw(message: &OutgoingMessage) -> Result<String, String> {
    let text_body = html_to_text(&message.html_body);

    let mut builder = MessageBuilder::new()
        .from((message.from_name.as_str(), message.from_email.as_str()))
        .subject(message.subject.as_str())
        .text_body(text_body)
        .html_body(message.html_body.as_str());

    let to: Vec<&str> = message.to.iter().map(String::as_str).collect();
    builder = builder.to(to);
    if !message.cc.is_empty() {
        let cc: Vec<&str> = message.cc.iter().map(String::as_str).collect();
        builder = builder.cc(cc);
    }
    if let Some(value) = &message.in_reply_to {
        builder = builder.in_reply_to(value.as_str());
    }
    if let Some(value) = &message.references {
        builder = builder.references(value.as_str());
    }
    for attachment in &message.attachments {
        builder = builder.attachment(
            attachment.mime.as_str(),
            attachment.filename.as_str(),
            attachment.data.as_slice(),
        );
    }

    let raw = builder.write_to_string().map_err(|e| e.to_string())?;
    Ok(URL_SAFE.encode(raw.as_bytes()))
}

/// A rough HTML-to-text reduction for the plain-text alternative: drop tags and
/// decode the handful of entities that matter. Not a full renderer — just a
/// legible fallback for clients that prefer text/plain.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut last_char = ' ';
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if in_tag => {}
            '\n' | '\r' | '\t' => {
                if last_char != ' ' {
                    out.push(' ');
                    last_char = ' ';
                }
            }
            _ => {
                out.push(c);
                last_char = c;
            }
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_text_strips_tags_and_decodes_entities() {
        assert_eq!(
            html_to_text("<p>Hello&nbsp;<b>Jane</b> &amp; Co</p>"),
            "Hello Jane & Co"
        );
    }

    #[test]
    fn build_raw_produces_decodable_base64url_with_recipients() {
        let msg = OutgoingMessage {
            from_name: "Me".into(),
            from_email: "me@example.com".into(),
            to: vec!["a@example.com".into(), "b@example.com".into()],
            cc: vec![],
            subject: "Hi".into(),
            html_body: "<p>Hello</p>".into(),
            in_reply_to: None,
            references: None,
            attachments: vec![],
        };
        let raw_b64 = build_raw(&msg).unwrap();
        let decoded = URL_SAFE.decode(raw_b64.as_bytes()).unwrap();
        let text = String::from_utf8_lossy(&decoded);
        assert!(text.contains("To:"));
        assert!(text.contains("a@example.com"));
        assert!(text.contains("Subject: Hi"));
        // Both alternatives present.
        assert!(text.contains("text/plain"));
        assert!(text.contains("text/html"));
    }
}
