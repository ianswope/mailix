//! The compose window: a libadwaita dialog wrapping a WebKit `contenteditable`
//! body. JavaScript is on here (unlike the read-only message view) so the
//! toolbar can drive `execCommand` and we can read the composed `innerHTML`
//! back on send. The From picker carries the account's native Gmail signature;
//! sending builds a MIME message (`crate::mime`) and posts it via
//! `messages.send` on a background thread.

use crate::config::GoogleConfig;
use crate::google::{gmail_api, oauth};
use crate::mime::{self, Attachment, OutgoingMessage};
use crate::secrets;
use crate::store::{Identity, Store};
use adw::prelude::*;
use gtk::glib;
use gtk::glib::clone;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use webkit6::prelude::*;

/// Pre-filled fields for the composer. Empty for a new message; populated for
/// reply/forward (recipients, quoted body, threading headers).
#[derive(Default)]
pub struct Prefill {
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub quoted_html: String,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub thread_id: Option<String>,
    pub prefer_account_id: Option<i64>,
}

/// Opens the composer. `identities` must be non-empty (the caller falls back to
/// account addresses if Gmail settings haven't synced yet). `on_sent` fires on
/// a successful send, so the caller can toast/refresh.
pub fn open(
    parent: &impl IsA<gtk::Widget>,
    config: GoogleConfig,
    identities: Vec<Identity>,
    prefill: Prefill,
    on_sent: Rc<dyn Fn()>,
) {
    if identities.is_empty() {
        return;
    }
    let parent_window = parent.root().and_downcast::<gtk::Window>();

    let dialog = adw::Dialog::builder()
        .title("New Message")
        .content_width(700)
        .content_height(640)
        .build();

    // From picker (with native signatures behind each identity).
    let from_model = gtk::StringList::new(&[]);
    for identity in &identities {
        let label = if identity.display_name.is_empty() {
            identity.email.clone()
        } else {
            format!("{} <{}>", identity.display_name, identity.email)
        };
        from_model.append(&label);
    }
    let default_index = prefill
        .prefer_account_id
        .and_then(|account| identities.iter().position(|i| i.account_id == account))
        .or_else(|| identities.iter().position(|i| i.is_default))
        .unwrap_or(0);
    let from_row = adw::ComboRow::builder()
        .title("From")
        .model(&from_model)
        .selected(default_index as u32)
        .build();

    let to_row = adw::EntryRow::builder().title("To").text(&prefill.to).build();
    let cc_row = adw::EntryRow::builder().title("Cc").text(&prefill.cc).build();
    let subject_row = adw::EntryRow::builder()
        .title("Subject")
        .text(&prefill.subject)
        .build();

    let fields = adw::PreferencesGroup::new();
    fields.add(&from_row);
    fields.add(&to_row);
    fields.add(&cc_row);
    fields.add(&subject_row);

    // Body editor.
    let web = webkit6::WebView::new();
    web.set_vexpand(true);
    web.set_hexpand(true);
    web.load_html(
        &editor_document(&identities[default_index].signature, &prefill.quoted_html),
        None,
    );

    // Formatting toolbar. Buttons don't take focus, so the editor keeps its
    // selection when a command runs.
    let bold = fmt_button("format-text-bold-symbolic", "Bold");
    let italic = fmt_button("format-text-italic-symbolic", "Italic");
    let underline = fmt_button("format-text-underline-symbolic", "Underline");
    let bullets = fmt_button("view-list-bullet-symbolic", "Bulleted list");
    let attach = gtk::Button::from_icon_name("mail-attachment-symbolic");
    attach.set_tooltip_text(Some("Attach files"));
    attach.set_focus_on_click(false);
    let attach_label = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    attach_label.add_css_class("dim-label");

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    toolbar.add_css_class("toolbar");
    toolbar.append(&bold);
    toolbar.append(&italic);
    toolbar.append(&underline);
    toolbar.append(&bullets);
    toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    toolbar.append(&attach);
    toolbar.append(&attach_label);

    for (button, command) in [
        (&bold, "bold"),
        (&italic, "italic"),
        (&underline, "underline"),
        (&bullets, "insertUnorderedList"),
    ] {
        button.connect_clicked(clone!(
            #[weak]
            web,
            move |_| {
                web.evaluate_javascript(
                    &format!("document.execCommand('{command}')"),
                    None,
                    None,
                    gtk::gio::Cancellable::NONE,
                    |_| {},
                );
            }
        ));
    }

    let error_label = gtk::Label::builder()
        .label("")
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    error_label.add_css_class("error");

    let web_frame = gtk::Frame::new(None);
    web_frame.set_child(Some(&web));
    web_frame.set_vexpand(true);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_top(8);
    content.set_margin_bottom(8);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.append(&error_label);
    content.append(&fields);
    content.append(&toolbar);
    content.append(&web_frame);

    let cancel = gtk::Button::with_label("Cancel");
    let send = gtk::Button::builder()
        .label("Send")
        .css_classes(["suggested-action"])
        .build();
    let header = adw::HeaderBar::new();
    header.pack_start(&cancel);
    header.pack_end(&send);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content));
    dialog.set_child(Some(&toolbar_view));

    let attachments: Rc<RefCell<Vec<Attachment>>> = Rc::new(RefCell::new(Vec::new()));

    cancel.connect_clicked(clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));

    attach.connect_clicked(clone!(
        #[strong]
        attachments,
        #[weak]
        attach_label,
        #[strong]
        parent_window,
        move |_| {
            let file_dialog = gtk::FileDialog::new();
            file_dialog.open_multiple(
                parent_window.as_ref(),
                gtk::gio::Cancellable::NONE,
                clone!(
                    #[strong]
                    attachments,
                    #[weak]
                    attach_label,
                    move |result| {
                        if let Ok(files) = result {
                            for i in 0..files.n_items() {
                                let Some(file) =
                                    files.item(i).and_downcast::<gtk::gio::File>()
                                else {
                                    continue;
                                };
                                let Some(path) = file.path() else { continue };
                                if let Ok(data) = std::fs::read(&path) {
                                    let filename = path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| "attachment".into());
                                    attachments.borrow_mut().push(Attachment {
                                        filename,
                                        mime: mime_from_path(&path),
                                        data,
                                    });
                                }
                            }
                            update_attach_label(&attach_label, &attachments.borrow());
                        }
                    }
                ),
            );
        }
    ));

    let reply_in_reply_to = prefill.in_reply_to.clone();
    let reply_references = prefill.references.clone();
    let reply_thread_id = prefill.thread_id.clone();

    send.connect_clicked(clone!(
        #[weak]
        dialog,
        #[weak]
        web,
        #[weak]
        send,
        #[weak]
        error_label,
        #[weak]
        from_row,
        #[weak]
        to_row,
        #[weak]
        cc_row,
        #[weak]
        subject_row,
        #[strong]
        attachments,
        #[strong]
        identities,
        #[strong]
        config,
        #[strong]
        on_sent,
        #[strong]
        reply_in_reply_to,
        #[strong]
        reply_references,
        #[strong]
        reply_thread_id,
        move |_| {
            error_label.set_visible(false);
            let Some(identity) = identities.get(from_row.selected() as usize).cloned() else {
                return;
            };
            let to = parse_addresses(&to_row.text());
            if to.is_empty() {
                error_label.set_text("Add at least one recipient in \u{201c}To\u{201d}.");
                error_label.set_visible(true);
                return;
            }
            send.set_sensitive(false);
            send.set_label("Sending\u{2026}");
            dispatch_send(SendContext {
                dialog: dialog.clone(),
                web: web.clone(),
                send: send.clone(),
                error_label: error_label.clone(),
                config: config.clone(),
                identity,
                to,
                cc: parse_addresses(&cc_row.text()),
                subject: subject_row.text().to_string(),
                in_reply_to: reply_in_reply_to.clone(),
                references: reply_references.clone(),
                thread_id: reply_thread_id.clone(),
                attachments: attachments.borrow().clone(),
                on_sent: on_sent.clone(),
            });
        }
    ));

    dialog.present(Some(parent));
    web.grab_focus();
}

/// Everything `dispatch_send` needs, bundled to avoid a giant argument list.
struct SendContext {
    dialog: adw::Dialog,
    web: webkit6::WebView,
    send: gtk::Button,
    error_label: gtk::Label,
    config: GoogleConfig,
    identity: Identity,
    to: Vec<String>,
    cc: Vec<String>,
    subject: String,
    in_reply_to: Option<String>,
    references: Option<String>,
    thread_id: Option<String>,
    attachments: Vec<Attachment>,
    on_sent: Rc<dyn Fn()>,
}

/// Reads the composed HTML out of the web view, then builds and sends the
/// message on a background thread, closing the dialog on success.
fn dispatch_send(ctx: SendContext) {
    let SendContext {
        dialog,
        web,
        send,
        error_label,
        config,
        identity,
        to,
        cc,
        subject,
        in_reply_to,
        references,
        thread_id,
        attachments,
        on_sent,
    } = ctx;

    web.clone().evaluate_javascript(
        "document.getElementById('editor').innerHTML",
        None,
        None,
        gtk::gio::Cancellable::NONE,
        move |result| {
            let html_body = result.map(|value| value.to_str().to_string()).unwrap_or_default();
            let account_id = identity.account_id;
            let message = OutgoingMessage {
                from_name: identity.display_name,
                from_email: identity.email,
                to,
                cc,
                subject,
                html_body,
                in_reply_to,
                references,
                attachments,
            };

            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let result = (|| -> Result<(), String> {
                    let store = Store::open().map_err(|e| e.to_string())?;
                    let account = store
                        .account(account_id)
                        .map_err(|e| e.to_string())?
                        .ok_or("account no longer exists")?;
                    let refresh = secrets::get(&account.token_key)
                        .map_err(|e| e.to_string())?
                        .ok_or("no saved credentials for this account")?;
                    let token =
                        oauth::refresh_access_token(&config, &refresh).map_err(|e| e.to_string())?;
                    let raw = mime::build_raw(&message)?;
                    gmail_api::send_message(&token, &raw, thread_id.as_deref())
                })();
                let _ = tx.send(result);
            });

            glib::timeout_add_local(
                Duration::from_millis(150),
                clone!(
                    #[strong]
                    dialog,
                    #[strong]
                    send,
                    #[strong]
                    error_label,
                    #[strong]
                    on_sent,
                    move || match rx.try_recv() {
                        Ok(Ok(())) => {
                            on_sent();
                            dialog.close();
                            glib::ControlFlow::Break
                        }
                        Ok(Err(error)) => {
                            error_label.set_text(&format!(
                                "Send failed: {}",
                                error.lines().next().unwrap_or("unknown error")
                            ));
                            error_label.set_visible(true);
                            send.set_sensitive(true);
                            send.set_label("Send");
                            glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                    }
                ),
            );
        },
    );
}

fn editor_document(signature: &str, quoted: &str) -> String {
    let signature_block = if signature.trim().is_empty() {
        String::new()
    } else {
        format!("<br><br><div class=\"mx-sig\">--<br>{signature}</div>")
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
         html,body{{margin:0;padding:0;background:#fff;}}\
         #editor{{font-family:-apple-system,'Cantarell','Segoe UI',sans-serif;font-size:14px;\
           line-height:1.5;color:#1a1a1a;padding:12px;min-height:240px;outline:none;}}\
         </style></head><body>\
         <div id=\"editor\" contenteditable=\"true\"><p><br></p>{signature_block}{quoted}</div>\
         </body></html>"
    )
}

fn fmt_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.set_tooltip_text(Some(tooltip));
    button.set_focus_on_click(false);
    button
}

fn parse_addresses(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn update_attach_label(label: &gtk::Label, attachments: &[Attachment]) {
    if attachments.is_empty() {
        label.set_text("");
        return;
    }
    let names: Vec<&str> = attachments.iter().map(|a| a.filename.as_str()).collect();
    label.set_text(&format!("{} attached: {}", attachments.len(), names.join(", ")));
}

fn mime_from_path(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("txt") | Some("log") => "text/plain",
        Some("csv") => "text/csv",
        Some("html") => "text/html",
        Some("zip") => "application/zip",
        Some("doc") => "application/msword",
        Some("docx") => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        Some("xls") => "application/vnd.ms-excel",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
    .to_string()
}
