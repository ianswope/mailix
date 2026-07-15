# Mailix

A native email app for Linux, built as a companion to [Calix](https://github.com/ianswope/calix).
Native GTK4 + libadwaita, in Rust, with direct Gmail sync across multiple
accounts and full-fidelity HTML rendering via WebKitGTK. Where Calix gave the
desktop a native calendar, Mailix aims to give it a native inbox — one that
speaks Gmail's own model (labels, threads, search, and native signatures)
rather than papering over it.

**Status: early days.** What works today:

- **Multiple Google accounts** merged into one inbox, synced straight from the
  Gmail API (labels, threads, and Gmail search under the hood).
- **HTML rendering** in an embedded WebKitGTK view, with **remote images
  blocked by default** (goodbye tracking pixels) behind a one-click "Load
  Images", JavaScript disabled, and links opened in your real browser.
- **Triage**: open-to-read, star, mark-unread, archive, and trash — applied
  locally and pushed to Gmail.
- **Compose & send**: a rich-text composer that pulls in your **native Gmail
  signatures**, with a From picker, attachments, and delivery via the Gmail
  API so sent mail threads correctly.
- **Offline**: accounts, threads, and message bodies are cached locally in
  SQLite; secrets live in the system keyring, never on disk.
- On [Omarchy](https://omarchy.org/), Mailix picks up the active theme's colors
  automatically, so it matches the rest of the desktop.

Not done yet: reply/forward, iCloud and generic IMAP/SMTP accounts, desktop
notifications, and packaging. See the roadmap below.

The chrome (sidebar, conversation list, dialogs) is all native libadwaita; the
embedded web engine is used *only* where it earns its keep — rendering the
message body and powering the composer. The engine layer (OAuth, keyring,
SQLite cache, sync) is shared in spirit with Calix.

## Building

Requires a Rust toolchain and the GTK4 (>= 4.14), libadwaita (>= 1.5), and
WebKitGTK 6.0 development headers.

- **Arch**: `gtk4`, `libadwaita`, `webkitgtk-6.0`
- **Debian/Ubuntu**: `libgtk-4-dev`, `libadwaita-1-dev`, `libwebkitgtk-6.0-dev`

```sh
cargo build
cargo test
cargo run
```

Mailix is written to build on Linux, the BSDs, and macOS (via Homebrew's GTK4 /
libadwaita / WebKitGTK), with secrets stored through Secret Service on
Linux/BSD and the Keychain on macOS.

## Connecting Google

Like Calix, Google requires every app to bring its own OAuth client — there's
no shared one. Setup takes about ten minutes:

1. Create a project at [console.cloud.google.com](https://console.cloud.google.com)
   and enable the **Gmail API** for it.
2. Under **Google Auth Platform → Audience**, set the app to External and add
   your own Google account under **Test users** (the app stays in "Testing,"
   which is fine for personal use — public verification is a separate, heavier
   process not needed here).
3. Under **Data Access**, add these scopes:
   - `https://www.googleapis.com/auth/gmail.modify`
   - `https://www.googleapis.com/auth/gmail.send`
   - `https://www.googleapis.com/auth/gmail.settings.basic`
4. Under **Clients**, create an OAuth client of type **Desktop app**. Copy the
   Client ID and Client Secret.
5. Create `~/.config/mailix/config.toml`:
   ```toml
   [google]
   client_id = "your-client-id.apps.googleusercontent.com"
   client_secret = "your-client-secret"
   ```
   This file lives outside the repo and is never committed — each user needs
   their own.
6. Run Mailix and click **Add Google** in the sidebar. It opens your browser
   for consent; once approved, the refresh token is saved to your system
   keyring (via Secret Service / Keychain), not to a file. Repeat for each
   account, and use **Sync** to refresh them.

If you already run Calix, you can reuse the same Google Cloud project — just
enable the Gmail API and add the scopes above.

## Architecture

- `src/config.rs` — reads `~/.config/mailix/config.toml` (the Google OAuth
  client).
- `src/secrets.rs` — cross-platform credential storage (refresh tokens, IMAP
  passwords) over the `keyring` crate.
- `src/store.rs` — the SQLite cache (accounts, labels, threads, messages,
  bodies, attachments, identities), WAL with a per-thread connection.
- `src/google/oauth.rs` — the OAuth2 + PKCE loopback sign-in (no embedded
  browser).
- `src/google/gmail_api.rs` — a thin REST client over the Gmail API v1.
- `src/google/sync.rs` — pulls labels, recent inbox threads, and send-as
  identities into the store.
- `src/render.rs` — sanitizes message HTML (`ammonia`) and builds the CSP-gated
  document loaded into the message view.
- `src/mime.rs` — builds outgoing MIME (`mail-builder`) for `messages.send`.
- `src/composer.rs` — the compose window (WebKit `contenteditable` + native
  signatures + attachments).
- `src/window.rs` — the three-pane shell (accounts / conversations / message),
  header actions, and background sync/action wiring.
- `src/omarchy.rs` — recolors libadwaita from the active Omarchy theme; a no-op
  elsewhere.

## Roadmap

- [x] Multi-account Gmail sign-in (OAuth + PKCE) and sync
- [x] Combined inbox, conversation list, and HTML message rendering
- [x] Remote-content blocking with per-message opt-in
- [x] Triage: read/unread, star, archive, trash
- [x] Compose & send with native Gmail signatures and attachments
- [ ] Reply / reply-all / forward
- [ ] iCloud and generic IMAP/SMTP accounts (shared engine)
- [ ] Unified-inbox refinements, incremental sync, desktop notifications
- [ ] Read-only view of native Gmail filters
- [ ] Packaging (Homebrew, AUR, Flatpak, macOS `.app`)

## License

MIT — see [LICENSE](LICENSE).
