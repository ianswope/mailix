//! Turns a message's raw HTML (or plain text) into a safe document for the
//! WebKitWebView. Two layers of defense:
//!
//! 1. **Structure** — `ammonia` strips `<script>`, event handlers, `<iframe>`,
//!    `<object>`, `<form>`, etc., while keeping the presentational markup email
//!    relies on (tables, inline `style`, `<font>`, …).
//! 2. **Network** — a Content-Security-Policy `<meta>` in the wrapper document
//!    is the actual gate on remote content. By default it forbids everything
//!    but inline styles and `cid:`/`data:` images, so tracking pixels and
//!    remote images never load. "Load remote images" simply re-renders with a
//!    CSP that also permits `https:`/`http:`. JavaScript is disabled on the web
//!    view regardless.
//!
//! The message area always renders on a light background (like Apple Mail /
//! Thunderbird), since the vast majority of HTML mail assumes one — this avoids
//! the dark-on-dark unreadability of theming the body.

/// One message rendered into the conversation view.
pub struct RenderMessage {
    pub from: String,
    pub date: String,
    /// The message body as HTML — already extracted (HTML part, or plain text
    /// wrapped via [`text_to_html`]).
    pub body_html: String,
}

/// Sanitizes a message's HTML body, keeping email's presentational markup.
pub fn sanitize(raw_html: &str) -> String {
    let mut builder = ammonia::Builder::new();
    builder
        .add_tags([
            "style", "font", "center", "span", "div", "table", "thead", "tbody", "tfoot", "tr",
            "td", "th", "col", "colgroup",
        ])
        // `<style>` is in ammonia's default `clean_content_tags` (content
        // discarded). We keep embedded CSS for email fidelity — it's safe here
        // because the document's CSP forbids remote fetches and JS is disabled.
        // `<script>` stays content-stripped.
        .rm_clean_content_tags(["style"])
        .add_generic_attributes([
            "style",
            "class",
            "id",
            "align",
            "valign",
            "dir",
            "lang",
            "title",
            "bgcolor",
            "color",
            "width",
            "height",
            "border",
            "cellpadding",
            "cellspacing",
            "colspan",
            "rowspan",
            "nowrap",
        ])
        .add_tag_attributes("img", ["src", "alt", "width", "height"])
        .add_tag_attributes("font", ["color", "face", "size"])
        .url_schemes(
            ["http", "https", "mailto", "cid", "data", "tel"]
                .into_iter()
                .collect(),
        )
        // Links open in the external browser (handled by the web view's policy
        // hook); mark them safe regardless.
        .link_rel(Some("noopener noreferrer nofollow"));
    builder.clean(raw_html).to_string()
}

/// Wraps plain text as minimal, safe HTML (preserving wrapping/whitespace).
pub fn text_to_html(text: &str) -> String {
    format!(
        "<pre style=\"white-space:pre-wrap;word-wrap:break-word;font-family:inherit;margin:0\">{}</pre>",
        escape(text)
    )
}

/// A cheap heuristic for whether a body would fetch remote resources, so the
/// UI knows whether to offer "Load remote images".
pub fn has_remote_content(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("http://") || lower.contains("https://")
}

/// Builds the full document loaded into the web view: a conversation of one or
/// more messages, each with a small sender/date header.
pub fn document(messages: &[RenderMessage], allow_remote: bool) -> String {
    let csp = if allow_remote {
        "default-src 'none'; img-src cid: data: https: http:; style-src 'unsafe-inline'; font-src data:"
    } else {
        "default-src 'none'; img-src cid: data:; style-src 'unsafe-inline'; font-src data:"
    };

    let mut sections = String::new();
    for message in messages {
        sections.push_str(&format!(
            "<div class=\"mx-msg\">\
               <div class=\"mx-head\">\
                 <span class=\"mx-from\">{}</span>\
                 <span class=\"mx-date\">{}</span>\
               </div>\
               <div class=\"mx-body\">{}</div>\
             </div>",
            escape(&message.from),
            escape(&message.date),
            message.body_html
        ));
    }

    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">\
         <base target=\"_blank\">\
         <style>\
           html,body{{margin:0;padding:0;background:#ffffff;color:#1a1a1a;\
             font-family:-apple-system,'Cantarell','Segoe UI',Roboto,sans-serif;\
             font-size:14px;line-height:1.5;}}\
           .mx-msg{{padding:16px 20px;border-bottom:1px solid rgba(0,0,0,0.08);}}\
           .mx-head{{display:flex;justify-content:space-between;gap:12px;\
             margin-bottom:12px;font-size:13px;}}\
           .mx-from{{font-weight:600;}}\
           .mx-date{{color:#707070;white-space:nowrap;}}\
           .mx-body{{overflow-wrap:break-word;}}\
           .mx-body img{{max-width:100%;height:auto;}}\
           .mx-body table{{max-width:100%;}}\
           a{{color:#1a73e8;}}\
         </style></head><body>{sections}</body></html>"
    )
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_scripts_but_keeps_tables_and_inline_style() {
        let dirty = r#"<table><tr><td style="color:red">Hi</td></tr></table><script>alert(1)</script>"#;
        let clean = sanitize(dirty);
        assert!(clean.contains("<table"));
        assert!(clean.contains("style"));
        assert!(!clean.contains("<script"));
        assert!(!clean.contains("alert"));
    }

    #[test]
    fn default_document_csp_blocks_remote_images() {
        let msgs = [RenderMessage {
            from: "a@b.com".into(),
            date: "2026-07-15".into(),
            body_html: "<img src=\"https://tracker.example/x.gif\">".into(),
        }];
        let doc = document(&msgs, false);
        assert!(doc.contains("img-src cid: data:;"));
        assert!(!doc.contains("img-src cid: data: https:"));
    }

    #[test]
    fn allow_remote_document_permits_https() {
        let doc = document(&[], true);
        assert!(doc.contains("img-src cid: data: https: http:"));
    }

    #[test]
    fn escape_neutralizes_angle_brackets_in_headers() {
        assert_eq!(escape("<b>&\"'"), "&lt;b&gt;&amp;&quot;&#39;");
    }
}
