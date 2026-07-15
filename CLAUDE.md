# Mailix — working agreement

A native email client: GTK4 + libadwaita (Rust, edition 2024), SQLite via
`rusqlite`, Gmail over the REST API (OAuth2 + PKCE).

## Test-driven development

This project is developed test-first. For any change that adds or alters
**behavior**, the loop is:

1. **Red** — write a failing test that pins down the behavior you want, and run
   it to watch it fail (`cargo test`). A test that passes before you touch the
   code proves nothing.
2. **Green** — write the minimum code to make it pass.
3. **Refactor** — clean up with the test as a safety net; keep it green.

Run the suite with `cargo test` — it's fast (all pure logic, no network). The
whole suite must be green before a change is considered done.

### What we test, and how to keep code testable

GTK widget wiring, network I/O, the OS keyring, and browser-spawning are not
unit-tested — they're driven by hand. Everything else is. The way we keep the
suite meaningful in a GUI app is to **push logic out of the widget callbacks
into pure functions** and test those:

- Parsing/serialization (Gmail JSON → structs, header lookup, MIME-tree body
  extraction, base64url) — see `google/gmail_api.rs`.
- Formatting/derivation (address parsing, sender display name, date strings,
  epoch↔RFC3339, MIME-type-from-extension) — see `composer.rs`, `window.rs`,
  `google/sync.rs`.
- Persistence round-trips against an in-memory SQLite — see `store.rs`.
- HTML sanitization + CSP — see `render.rs`, `mime.rs`.

When a new behavior lives inside a GTK callback, first extract the decision or
transformation into a free function that takes plain data and returns plain
data, write the test against *that*, then call it from the callback. If a
function needs the network/keyring/filesystem to be tested, that's usually a
sign the pure part should be split out (as `Config::from_toml` is split from
`Config::load`).

Tests live in a `#[cfg(test)] mod tests` block at the bottom of the module they
cover (this is the repo convention — there is no top-level `tests/` dir).
Deserialization-backed tests build fixtures from JSON string literals so they
exercise the serde field renames too.

## Layout

- `store.rs` — SQLite schema + all queries; the local cache of accounts,
  threads, messages, labels, identities.
- `google/` — `oauth.rs` (sign-in/refresh), `gmail_api.rs` (REST client + wire
  types), `sync.rs` (fetch → store).
- `render.rs` — sanitize remote HTML + build the CSP for the WebKit view.
- `mime.rs` — build outgoing RFC 5322 messages for `messages.send`.
- `composer.rs`, `window.rs` — GTK UI.
- `secrets.rs` — refresh tokens in the OS keyring. `config.rs` — user OAuth
  client. `omarchy.rs`/`style.rs` — theming. `util.rs` — cross-platform seams.
