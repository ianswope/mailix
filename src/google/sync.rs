//! Fetches a Gmail account's labels and recent inbox threads and upserts them
//! into the local store — the read-path analogue of calix's `google/sync.rs`.
//! Bodies are NOT pulled here; they're fetched lazily when a thread is opened
//! (see `crate::window`), so initial sync stays fast.
//!
//! Blocks on network I/O — call from a background thread with a fresh
//! `Store::open()` (rusqlite connections aren't `Send`).

use crate::google::gmail_api::{self, ApiMessage};
use crate::store::{Identity, NewMessage, NewThread, Store};
use std::collections::HashMap;

const INITIAL_THREAD_LIMIT: u32 = 50;

/// A stable identity for a Google account: its primary email (used both as the
/// display name and as the keyring/account key).
pub fn account_identity(access_token: &str) -> Result<(String, String), String> {
    let email = gmail_api::get_profile(access_token)?.email_address;
    Ok((email.clone(), email))
}

/// Syncs labels + the most recent inbox threads into the store. Returns the
/// number of threads synced, for user-facing feedback.
pub fn sync_account(access_token: &str, store: &Store, account_id: i64) -> Result<usize, String> {
    // Labels first, keeping a remote-id -> local-id map to attach to messages.
    let mut label_ids: HashMap<String, i64> = HashMap::new();
    for label in gmail_api::list_labels(access_token)? {
        let id = store
            .upsert_label(account_id, &label.id, &label.name, label.kind(), label.color().as_deref())
            .map_err(|e| e.to_string())?;
        label_ids.insert(label.id, id);
    }

    // Send-as identities (with native signatures) for the composer.
    let identities: Vec<Identity> = gmail_api::list_send_as(access_token)?
        .into_iter()
        .map(|s| Identity {
            account_id,
            email: s.email,
            display_name: s.display_name.unwrap_or_default(),
            signature: s.signature.unwrap_or_default(),
            is_default: s.is_default || s.is_primary,
        })
        .collect();
    store
        .replace_identities(account_id, &identities)
        .map_err(|e| e.to_string())?;

    let thread_refs = gmail_api::list_thread_ids(access_token, "in:inbox", INITIAL_THREAD_LIMIT)?;
    for thread_ref in &thread_refs {
        let thread = match gmail_api::get_thread_metadata(access_token, &thread_ref.id) {
            Ok(thread) => thread,
            Err(error) => {
                eprintln!("mailix: skipping thread {}: {error}", thread_ref.id);
                continue;
            }
        };
        let (Some(first), Some(last)) = (thread.messages.first(), thread.messages.last()) else {
            continue;
        };

        let last_ms = thread
            .messages
            .iter()
            .filter_map(ApiMessage::internal_date_ms)
            .max()
            .unwrap_or(0);
        let local_thread_id = store
            .upsert_thread(&NewThread {
                account_id,
                remote_thread_id: thread_ref.id.clone(),
                subject: first
                    .header("Subject")
                    .filter(|s| !s.is_empty())
                    .unwrap_or("(no subject)")
                    .to_string(),
                snippet: thread_ref
                    .snippet
                    .clone()
                    .or_else(|| last.snippet.clone())
                    .unwrap_or_default(),
                sender: last.header("From").unwrap_or("").to_string(),
                last_date: iso_from_ms(last_ms),
                unread: thread.messages.iter().any(ApiMessage::is_unread),
                starred: thread
                    .messages
                    .iter()
                    .any(|m| m.label_ids.iter().any(|l| l == "STARRED")),
                has_attachments: false,
            })
            .map_err(|e| e.to_string())?;

        for message in &thread.messages {
            let message_id = store
                .upsert_message(&NewMessage {
                    account_id,
                    thread_id: local_thread_id,
                    remote_msg_id: message.id.clone(),
                    from_addr: message.header("From").map(str::to_string),
                    to_addrs: message.header("To").map(str::to_string),
                    cc_addrs: message.header("Cc").map(str::to_string),
                    date: message.internal_date_ms().map(iso_from_ms),
                    subject: message.header("Subject").map(str::to_string),
                    snippet: message.snippet.clone(),
                    seen: !message.is_unread(),
                })
                .map_err(|e| e.to_string())?;

            let locals: Vec<i64> = message
                .label_ids
                .iter()
                .filter_map(|remote| label_ids.get(remote).copied())
                .collect();
            store
                .set_message_labels(message_id, &locals)
                .map_err(|e| e.to_string())?;
        }
    }

    Ok(thread_refs.len())
}

/// Epoch-milliseconds -> RFC 3339 UTC, which sorts lexically (so the store can
/// `ORDER BY last_date`) and stays human-readable.
fn iso_from_ms(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}
