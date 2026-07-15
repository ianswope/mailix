use crate::composer;
use crate::config::{Config, GoogleConfig};
use crate::google::{self, gmail_api, oauth};
use crate::render::{self, RenderMessage};
use crate::secrets;
use crate::store::{Identity, NewAccount, Store, ThreadRow};
use crate::util;
use adw::prelude::*;
use gtk::glib;
use gtk::glib::clone;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use webkit6::prelude::*;

/// The conversation currently shown in the message pane. Identity fields are
/// set the moment a row is opened (so the header actions have a target);
/// `messages`/`has_remote` fill in once the bodies load, and let "Load Images"
/// re-render with remote content allowed.
struct CurrentThread {
    thread_id: i64,
    account_id: i64,
    remote_thread_id: String,
    starred: bool,
    messages: Vec<RenderMessage>,
    has_remote: bool,
}

/// A conversation-level label mutation, run on a background thread.
enum ThreadOp {
    Modify { add: Vec<String>, remove: Vec<String> },
    Trash,
}

/// Shared widget/state handle passed to the interactive handlers, so they don't
/// need a long parameter list — mirrors calix's `Ui`.
struct Ui {
    store: Rc<Store>,
    google_config: Option<GoogleConfig>,
    toast_overlay: adw::ToastOverlay,
    accounts_list: gtk::ListBox,
    thread_list: gtk::ListBox,
    threads: RefCell<Vec<ThreadRow>>,
    message_title: gtk::Label,
    message_web: webkit6::WebView,
    remote_banner: gtk::Revealer,
    current: RefCell<Option<CurrentThread>>,
    star_button: gtk::ToggleButton,
    unread_button: gtk::Button,
    archive_button: gtk::Button,
    trash_button: gtk::Button,
    // Guards the star toggle handler while we set its state programmatically,
    // so opening a thread doesn't fire a spurious star/unstar API call.
    star_updating: Cell<bool>,
}

impl Ui {
    fn toast(&self, message: &str) {
        self.toast_overlay
            .add_toast(adw::Toast::new(&glib::markup_escape_text(message)));
    }

    /// Repopulates the sidebar's account list from the store.
    fn reload_accounts(&self) {
        clear(&self.accounts_list);
        let accounts = self.store.list_accounts().unwrap_or_default();
        if accounts.is_empty() {
            let empty = gtk::Label::builder()
                .label("No accounts yet")
                .xalign(0.0)
                .build();
            empty.add_css_class("dim-label");
            empty.set_margin_start(12);
            empty.set_margin_top(4);
            empty.set_margin_bottom(4);
            self.accounts_list.append(&empty);
            return;
        }
        for account in &accounts {
            let row = adw::ActionRow::builder()
                .title(&account.email)
                .subtitle(&account.provider)
                .build();
            self.accounts_list.append(&row);
        }
    }

    /// Rebuilds the conversation list from the store's cached threads.
    fn reload_thread_list(&self) {
        let threads = self.store.list_inbox_threads().unwrap_or_default();
        clear(&self.thread_list);
        for thread in &threads {
            self.thread_list.append(&thread_row_widget(thread));
        }
        *self.threads.borrow_mut() = threads;
    }

    /// (Re)renders the open conversation into the web view; toggles the
    /// remote-content banner based on whether images are being withheld.
    fn render_current(&self, allow_remote: bool) {
        if let Some(current) = self.current.borrow().as_ref() {
            let html = render::document(&current.messages, allow_remote);
            self.message_web.load_html(&html, None);
            self.remote_banner
                .set_reveal_child(current.has_remote && !allow_remote);
        }
    }

    fn set_actions_enabled(&self, enabled: bool) {
        self.star_button.set_sensitive(enabled);
        self.unread_button.set_sensitive(enabled);
        self.archive_button.set_sensitive(enabled);
        self.trash_button.set_sensitive(enabled);
    }

    /// Sets the star toggle's visual state without triggering its handler.
    fn set_star_state(&self, starred: bool) {
        self.star_updating.set(true);
        self.star_button.set_active(starred);
        self.star_button
            .set_icon_name(if starred { "starred-symbolic" } else { "non-starred-symbolic" });
        self.star_updating.set(false);
    }

    /// Empties the message pane back to its no-selection state.
    fn clear_message_pane(&self) {
        *self.current.borrow_mut() = None;
        self.message_title.set_text("");
        self.message_web.load_html("<html><body></body></html>", None);
        self.remote_banner.set_reveal_child(false);
        self.set_actions_enabled(false);
    }
}

pub fn build(app: &adw::Application) {
    let store = Rc::new(Store::open().expect("open the Mailix data store"));
    let config = Config::load();

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(1250)
        .default_height(820)
        .build();
    window.set_title(Some("Mailix"));

    // --- Accounts sidebar ---
    let accounts_list = gtk::ListBox::new();
    accounts_list.add_css_class("navigation-sidebar");
    accounts_list.set_selection_mode(gtk::SelectionMode::None);

    let add_google_button = sidebar_action_button("Add Google", "Connect a Google account");
    let add_icloud_button = sidebar_action_button("Add iCloud", "Connect an iCloud account");
    let add_imap_button = sidebar_action_button("Add IMAP", "Connect any IMAP/SMTP mailbox");

    let sidebar_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sidebar_box.add_css_class("mailbox-sidebar");
    let accounts_heading = gtk::Label::builder().label("ACCOUNTS").xalign(0.0).build();
    accounts_heading.add_css_class("sidebar-section-label");
    sidebar_box.append(&accounts_heading);
    sidebar_box.append(&accounts_list);
    let actions = gtk::Box::new(gtk::Orientation::Vertical, 6);
    actions.set_margin_top(8);
    actions.set_margin_start(8);
    actions.set_margin_end(8);
    actions.set_margin_bottom(8);
    actions.append(&add_google_button);
    actions.append(&add_icloud_button);
    actions.append(&add_imap_button);
    sidebar_box.append(&actions);

    let sidebar_scroller = gtk::ScrolledWindow::builder()
        .child(&sidebar_box)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let sidebar_toolbar = adw::ToolbarView::new();
    let sidebar_header = adw::HeaderBar::new();
    sidebar_header.set_title_widget(Some(&adw::WindowTitle::new("Mailboxes", "")));
    sidebar_toolbar.add_top_bar(&sidebar_header);
    sidebar_toolbar.set_content(Some(&sidebar_scroller));
    let sidebar_page = adw::NavigationPage::builder()
        .title("Mailboxes")
        .child(&sidebar_toolbar)
        .build();

    // --- Thread list (middle pane) ---
    let thread_list = gtk::ListBox::new();
    thread_list.set_selection_mode(gtk::SelectionMode::Single);
    let thread_scroller = gtk::ScrolledWindow::builder()
        .child(&thread_list)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();

    let refresh_button = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh_button.set_tooltip_text(Some("Sync"));
    let compose_button = gtk::Button::from_icon_name("mail-message-new-symbolic");
    compose_button.set_tooltip_text(Some("Compose"));

    let inbox_header = adw::HeaderBar::new();
    inbox_header.set_title_widget(Some(&adw::WindowTitle::new("Inbox", "")));
    inbox_header.pack_start(&refresh_button);
    inbox_header.pack_end(&compose_button);
    let inbox_toolbar = adw::ToolbarView::new();
    inbox_toolbar.add_top_bar(&inbox_header);
    inbox_toolbar.set_content(Some(&thread_scroller));
    let inbox_page = adw::NavigationPage::builder()
        .title("Inbox")
        .child(&inbox_toolbar)
        .build();

    // --- Message pane (right) ---
    let settings = webkit6::Settings::new();
    settings.set_enable_javascript(false);
    settings.set_enable_javascript_markup(false);
    let message_web = webkit6::WebView::builder().settings(&settings).build();
    message_web.set_vexpand(true);
    message_web.set_hexpand(true);

    let remote_label = gtk::Label::builder()
        .label("Remote images were blocked to protect your privacy.")
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .build();
    let load_images_button = gtk::Button::with_label("Load Images");
    let banner_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    banner_box.add_css_class("remote-content-banner");
    banner_box.append(&remote_label);
    banner_box.append(&load_images_button);
    let remote_banner = gtk::Revealer::new();
    remote_banner.set_child(Some(&banner_box));
    remote_banner.set_reveal_child(false);

    let message_body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    message_body.append(&remote_banner);
    message_body.append(&message_web);

    let message_title = gtk::Label::builder()
        .label("")
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    message_title.add_css_class("heading");

    // Message-level actions (disabled until a conversation is open). No Adwaita
    // "archive" symbolic exists, so that one is a text button.
    let star_button = gtk::ToggleButton::new();
    star_button.set_icon_name("non-starred-symbolic");
    star_button.set_tooltip_text(Some("Star"));
    let unread_button = gtk::Button::from_icon_name("mail-unread-symbolic");
    unread_button.set_tooltip_text(Some("Mark unread"));
    let archive_button = gtk::Button::with_label("Archive");
    archive_button.set_tooltip_text(Some("Archive (remove from Inbox)"));
    let trash_button = gtk::Button::from_icon_name("user-trash-symbolic");
    trash_button.set_tooltip_text(Some("Move to Trash"));
    star_button.set_sensitive(false);
    unread_button.set_sensitive(false);
    archive_button.set_sensitive(false);
    trash_button.set_sensitive(false);

    let message_header = adw::HeaderBar::new();
    message_header.set_title_widget(Some(&message_title));
    // pack_end stacks right-to-left, so this reads: [unread] [archive] [trash] [star]
    message_header.pack_end(&star_button);
    message_header.pack_end(&trash_button);
    message_header.pack_end(&archive_button);
    message_header.pack_end(&unread_button);
    let message_toolbar = adw::ToolbarView::new();
    message_toolbar.add_top_bar(&message_header);

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&message_body));
    message_toolbar.set_content(Some(&toast_overlay));
    let message_page = adw::NavigationPage::builder()
        .title("Message")
        .child(&message_toolbar)
        .build();

    // --- Nested split views: accounts | (threads | message) ---
    let inner_split = adw::NavigationSplitView::new();
    inner_split.set_sidebar(Some(&inbox_page));
    inner_split.set_content(Some(&message_page));
    inner_split.set_min_sidebar_width(300.0);
    inner_split.set_max_sidebar_width(420.0);
    let inner_page = adw::NavigationPage::builder()
        .title("Mail")
        .child(&inner_split)
        .build();

    let outer_split = adw::NavigationSplitView::new();
    outer_split.set_sidebar(Some(&sidebar_page));
    outer_split.set_content(Some(&inner_page));
    outer_split.set_min_sidebar_width(220.0);
    outer_split.set_max_sidebar_width(300.0);

    window.set_content(Some(&outer_split));

    let ui = Rc::new(Ui {
        store,
        google_config: config.google.clone(),
        toast_overlay,
        accounts_list,
        thread_list: thread_list.clone(),
        threads: RefCell::new(Vec::new()),
        message_title,
        message_web: message_web.clone(),
        remote_banner,
        current: RefCell::new(None),
        star_button: star_button.clone(),
        unread_button: unread_button.clone(),
        archive_button: archive_button.clone(),
        trash_button: trash_button.clone(),
        star_updating: Cell::new(false),
    });

    // --- Wire handlers ---
    add_google_button.connect_clicked(clone!(
        #[strong]
        ui,
        #[weak]
        add_google_button,
        move |_| add_google(&ui, &add_google_button)
    ));
    for button in [&add_icloud_button, &add_imap_button] {
        button.connect_clicked(clone!(
            #[strong]
            ui,
            move |_| ui.toast("IMAP/iCloud account setup arrives in Phase 5.")
        ));
    }
    refresh_button.connect_clicked(clone!(
        #[strong]
        ui,
        #[weak]
        refresh_button,
        move |_| sync_all(&ui, &refresh_button)
    ));
    compose_button.connect_clicked(clone!(
        #[strong]
        ui,
        #[weak]
        window,
        move |_| {
            let Some(config) = ui.google_config.clone() else {
                ui.toast("Add a Google OAuth client to ~/.config/mailix/config.toml first.");
                return;
            };
            let identities = compose_identities(&ui.store);
            if identities.is_empty() {
                ui.toast("Connect a Google account before composing.");
                return;
            }
            let on_sent: Rc<dyn Fn()> = {
                let ui = ui.clone();
                Rc::new(move || ui.toast("Message sent"))
            };
            composer::open(
                &window,
                config,
                identities,
                composer::Prefill::default(),
                on_sent,
            );
        }
    ));
    thread_list.connect_row_activated(clone!(
        #[strong]
        ui,
        move |_, row| open_thread(&ui, row.index())
    ));
    load_images_button.connect_clicked(clone!(
        #[strong]
        ui,
        move |_| ui.render_current(true)
    ));
    unread_button.connect_clicked(clone!(
        #[strong]
        ui,
        move |_| mark_current_unread(&ui)
    ));
    archive_button.connect_clicked(clone!(
        #[strong]
        ui,
        move |_| remove_current(&ui, ThreadOp::Modify { add: vec![], remove: vec!["INBOX".into()] }, "Archived")
    ));
    trash_button.connect_clicked(clone!(
        #[strong]
        ui,
        move |_| remove_current(&ui, ThreadOp::Trash, "Moved to Trash")
    ));
    star_button.connect_toggled(clone!(
        #[strong]
        ui,
        move |button| {
            if ui.star_updating.get() {
                return;
            }
            toggle_current_star(&ui, button.is_active());
        }
    ));

    // Open http(s)/mailto links in the external browser instead of navigating
    // the message view; everything else (the about:blank load_html) proceeds.
    message_web.connect_decide_policy(|_web, decision, decision_type| {
        use webkit6::PolicyDecisionType;
        if matches!(
            decision_type,
            PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction
        ) && let Some(nav) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>()
            && let Some(uri) = nav
                .navigation_action()
                .and_then(|action| action.request())
                .and_then(|request| request.uri())
        {
            let uri = uri.to_string();
            if uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("mailto:")
            {
                let _ = util::open_in_browser(&uri);
                decision.ignore();
                return true;
            }
        }
        false
    });

    ui.reload_accounts();
    ui.reload_thread_list();
    window.present();
}

/// Runs the interactive Google OAuth flow on a background thread, stores the
/// account + refresh token, and does an initial sync. Mirrors calix's
/// `add_google_account`.
fn add_google(ui: &Rc<Ui>, add_button: &gtk::Button) {
    let Some(config) = ui.google_config.clone() else {
        ui.toast("Add a Google OAuth client to ~/.config/mailix/config.toml first — see the README.");
        return;
    };

    add_button.set_sensitive(false);
    add_button.set_label("Connecting…");

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = (|| -> Result<(String, usize), String> {
            let tokens = oauth::sign_in(&config).map_err(|e| e.to_string())?;
            let (email, display_name) = google::sync::account_identity(&tokens.access_token)?;
            let token_key = secrets::google_refresh_key(&email);
            secrets::set(&token_key, &tokens.refresh_token).map_err(|e| e.to_string())?;
            // Each background worker opens its own connection (rusqlite isn't Send).
            let store = Store::open().map_err(|e| e.to_string())?;
            let account_id = store
                .insert_account(&NewAccount {
                    provider: "gmail".into(),
                    email: email.clone(),
                    display_name,
                    token_key,
                    imap_host: None,
                    imap_port: None,
                    smtp_host: None,
                    smtp_port: None,
                    use_ssl: true,
                })
                .map_err(|e| e.to_string())?;
            let count = google::sync::sync_account(&tokens.access_token, &store, account_id)?;
            Ok((email, count))
        })();
        let _ = tx.send(result);
    });

    glib::timeout_add_local(
        Duration::from_millis(200),
        clone!(
            #[strong]
            ui,
            #[strong]
            add_button,
            move || match rx.try_recv() {
                Ok(Ok((email, count))) => {
                    ui.toast(&format!("Added {email} — synced {count} conversation(s)"));
                    reset_add_button(&add_button);
                    ui.reload_accounts();
                    ui.reload_thread_list();
                    glib::ControlFlow::Break
                }
                Ok(Err(error)) => {
                    ui.toast(&format!("Google connect failed: {}", first_line(&error)));
                    reset_add_button(&add_button);
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    reset_add_button(&add_button);
                    glib::ControlFlow::Break
                }
            }
        ),
    );
}

fn reset_add_button(button: &gtk::Button) {
    button.set_label("Add Google");
    button.set_sensitive(true);
}

/// Refreshes every connected Gmail account on a background thread.
fn sync_all(ui: &Rc<Ui>, sync_button: &gtk::Button) {
    let Some(config) = ui.google_config.clone() else {
        ui.toast("No Google account is configured yet.");
        return;
    };
    sync_button.set_sensitive(false);

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = (|| -> Result<usize, String> {
            let store = Store::open().map_err(|e| e.to_string())?;
            let accounts = store.list_accounts().map_err(|e| e.to_string())?;
            let mut total = 0;
            for account in accounts.iter().filter(|a| a.provider == "gmail") {
                let Some(refresh) =
                    secrets::get(&account.token_key).map_err(|e| e.to_string())?
                else {
                    continue;
                };
                let token = oauth::refresh_access_token(&config, &refresh)
                    .map_err(|e| e.to_string())?;
                total += google::sync::sync_account(&token, &store, account.id)?;
            }
            Ok(total)
        })();
        let _ = tx.send(result);
    });

    glib::timeout_add_local(
        Duration::from_millis(200),
        clone!(
            #[strong]
            ui,
            #[strong]
            sync_button,
            move || match rx.try_recv() {
                Ok(Ok(count)) => {
                    ui.toast(&format!("Synced {count} conversation(s)"));
                    sync_button.set_sensitive(true);
                    ui.reload_thread_list();
                    glib::ControlFlow::Break
                }
                Ok(Err(error)) => {
                    ui.toast(&format!("Sync failed: {}", first_line(&error)));
                    sync_button.set_sensitive(true);
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    sync_button.set_sensitive(true);
                    glib::ControlFlow::Break
                }
            }
        ),
    );
}

/// Loads the selected conversation's messages (from cache, or fetched from
/// Gmail on a background thread) and renders them.
fn open_thread(ui: &Rc<Ui>, index: i32) {
    let Some(thread) = ui.threads.borrow().get(index as usize).cloned() else {
        return;
    };
    let Some(config) = ui.google_config.clone() else {
        return;
    };

    // Set identity up front so the header actions have a target immediately.
    *ui.current.borrow_mut() = Some(CurrentThread {
        thread_id: thread.id,
        account_id: thread.account_id,
        remote_thread_id: thread.remote_thread_id.clone(),
        starred: thread.starred,
        messages: Vec::new(),
        has_remote: false,
    });
    ui.message_title.set_text(&thread.subject);
    ui.set_actions_enabled(true);
    ui.set_star_state(thread.starred);
    ui.message_web.load_html(&loading_page(), None);
    ui.remote_banner.set_reveal_child(false);

    // Opening a conversation marks it read — Gmail-native behavior.
    if thread.unread {
        let _ = ui.store.set_thread_unread(thread.id, false);
        if let Some(row) = ui.thread_list.selected_row()
            && let Some(child) = row.child()
        {
            child.remove_css_class("unread");
        }
        if let Some(t) = ui.threads.borrow_mut().get_mut(index as usize) {
            t.unread = false;
        }
        let rx = run_op(
            config.clone(),
            thread.account_id,
            thread.remote_thread_id.clone(),
            ThreadOp::Modify {
                add: vec![],
                remove: vec!["UNREAD".into()],
            },
        );
        poll_error_only(ui, rx);
    }

    let opened_id = thread.id;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(load_thread_messages(&config, &thread));
    });

    glib::timeout_add_local(
        Duration::from_millis(80),
        clone!(
            #[strong]
            ui,
            move || match rx.try_recv() {
                Ok(Ok((messages, has_remote))) => {
                    // Skip if the user has since opened a different conversation.
                    let mut applied = false;
                    {
                        let mut current = ui.current.borrow_mut();
                        if let Some(cur) = current.as_mut()
                            && cur.thread_id == opened_id
                        {
                            cur.messages = messages;
                            cur.has_remote = has_remote;
                            applied = true;
                        }
                    }
                    if applied {
                        ui.render_current(false);
                    }
                    glib::ControlFlow::Break
                }
                Ok(Err(error)) => {
                    ui.message_web.load_html(&error_page(&error), None);
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        ),
    );
}

/// Restores the UNREAD state on the open conversation.
fn mark_current_unread(ui: &Rc<Ui>) {
    let Some((thread_id, account_id, remote_thread_id)) = current_identity(ui) else {
        return;
    };
    let Some(config) = ui.google_config.clone() else {
        return;
    };
    let _ = ui.store.set_thread_unread(thread_id, true);
    if let Some(row) = ui.thread_list.selected_row()
        && let Some(child) = row.child()
    {
        child.add_css_class("unread");
    }
    if let Some(t) = ui.threads.borrow_mut().iter_mut().find(|t| t.id == thread_id) {
        t.unread = true;
    }
    ui.toast("Marked unread");
    let rx = run_op(
        config,
        account_id,
        remote_thread_id,
        ThreadOp::Modify {
            add: vec!["UNREAD".into()],
            remove: vec![],
        },
    );
    poll_error_only(ui, rx);
}

/// Stars/unstars the open conversation.
fn toggle_current_star(ui: &Rc<Ui>, active: bool) {
    let Some((thread_id, account_id, remote_thread_id)) = current_identity(ui) else {
        return;
    };
    let Some(config) = ui.google_config.clone() else {
        return;
    };
    ui.star_button
        .set_icon_name(if active { "starred-symbolic" } else { "non-starred-symbolic" });
    let _ = ui.store.set_thread_starred(thread_id, active);
    if let Some(cur) = ui.current.borrow_mut().as_mut() {
        cur.starred = active;
    }
    if let Some(t) = ui.threads.borrow_mut().iter_mut().find(|t| t.id == thread_id) {
        t.starred = active;
    }
    let op = if active {
        ThreadOp::Modify {
            add: vec!["STARRED".into()],
            remove: vec![],
        }
    } else {
        ThreadOp::Modify {
            add: vec![],
            remove: vec!["STARRED".into()],
        }
    };
    let rx = run_op(config, account_id, remote_thread_id, op);
    poll_error_only(ui, rx);
}

/// Archives or trashes the open conversation. Optimistically drops it locally
/// and rebuilds the list (so row indices stay in sync); if the server call
/// fails, it returns on the next sync.
fn remove_current(ui: &Rc<Ui>, op: ThreadOp, done_msg: &'static str) {
    let Some((thread_id, account_id, remote_thread_id)) = current_identity(ui) else {
        return;
    };
    let Some(config) = ui.google_config.clone() else {
        return;
    };
    let _ = ui.store.delete_thread(thread_id);
    ui.clear_message_pane();
    ui.reload_thread_list();

    let rx = run_op(config, account_id, remote_thread_id, op);
    glib::timeout_add_local(
        Duration::from_millis(150),
        clone!(
            #[strong]
            ui,
            move || match rx.try_recv() {
                Ok(Ok(())) => {
                    ui.toast(done_msg);
                    glib::ControlFlow::Break
                }
                Ok(Err(error)) => {
                    ui.toast(&format!(
                        "Server rejected it ({}); it'll return on next sync.",
                        first_line(&error)
                    ));
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        ),
    );
}

fn current_identity(ui: &Rc<Ui>) -> Option<(i64, i64, String)> {
    ui.current
        .borrow()
        .as_ref()
        .map(|c| (c.thread_id, c.account_id, c.remote_thread_id.clone()))
}

/// Identities for the composer's From picker. Falls back to the connected
/// accounts' own addresses when Gmail's `sendAs` settings haven't synced yet.
fn compose_identities(store: &Store) -> Vec<Identity> {
    let identities = store.list_identities().unwrap_or_default();
    if !identities.is_empty() {
        return identities;
    }
    store
        .list_accounts()
        .unwrap_or_default()
        .into_iter()
        .map(|account| Identity {
            account_id: account.id,
            email: account.email,
            display_name: account.display_name,
            signature: String::new(),
            is_default: false,
        })
        .collect()
}

/// Spawns a conversation-level Gmail mutation on a background thread, returning
/// a channel that yields the result.
fn run_op(
    config: GoogleConfig,
    account_id: i64,
    remote_thread_id: String,
    op: ThreadOp,
) -> mpsc::Receiver<Result<(), String>> {
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
            let token = oauth::refresh_access_token(&config, &refresh).map_err(|e| e.to_string())?;
            match op {
                ThreadOp::Modify { add, remove } => {
                    let add_refs: Vec<&str> = add.iter().map(String::as_str).collect();
                    let remove_refs: Vec<&str> = remove.iter().map(String::as_str).collect();
                    gmail_api::modify_thread(&token, &remote_thread_id, &add_refs, &remove_refs)
                }
                ThreadOp::Trash => gmail_api::trash_thread(&token, &remote_thread_id),
            }
        })();
        let _ = tx.send(result);
    });
    rx
}

/// Polls a background op, surfacing only failures as a toast (the optimistic UI
/// already reflects success).
fn poll_error_only(ui: &Rc<Ui>, rx: mpsc::Receiver<Result<(), String>>) {
    glib::timeout_add_local(
        Duration::from_millis(150),
        clone!(
            #[strong]
            ui,
            move || match rx.try_recv() {
                Ok(Ok(())) => glib::ControlFlow::Break,
                Ok(Err(error)) => {
                    ui.toast(&format!("Action failed: {}", first_line(&error)));
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        ),
    );
}

/// Background worker: returns the rendered (sanitized) messages for a thread
/// plus whether any carries remote content. Uses cached bodies when present,
/// otherwise fetches the full thread from Gmail and caches the bodies.
fn load_thread_messages(
    config: &GoogleConfig,
    thread: &ThreadRow,
) -> Result<(Vec<RenderMessage>, bool), String> {
    let store = Store::open().map_err(|e| e.to_string())?;
    let rows = store
        .messages_in_thread(thread.id)
        .map_err(|e| e.to_string())?;

    // Fully cached: render offline, no network.
    if !rows.is_empty() && rows.iter().all(|m| m.has_body) {
        let mut rendered = Vec::new();
        let mut has_remote = false;
        for row in &rows {
            let body = store
                .get_body(row.id)
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            let body_html = body
                .sanitized_html
                .or_else(|| body.html.as_deref().map(render::sanitize))
                .or_else(|| body.text_plain.as_deref().map(render::text_to_html))
                .unwrap_or_else(no_content);
            has_remote |= render::has_remote_content(&body_html);
            rendered.push(RenderMessage {
                from: row.from_addr.clone().unwrap_or_default(),
                date: row.date.as_deref().map(msg_date).unwrap_or_default(),
                body_html,
            });
        }
        return Ok((rendered, has_remote));
    }

    // Fetch full thread and cache the bodies.
    let account = store
        .account(thread.account_id)
        .map_err(|e| e.to_string())?
        .ok_or("account no longer exists")?;
    let refresh = secrets::get(&account.token_key)
        .map_err(|e| e.to_string())?
        .ok_or("no saved credentials for this account")?;
    let token = oauth::refresh_access_token(config, &refresh).map_err(|e| e.to_string())?;
    let api_thread = gmail_api::get_thread_full(&token, &thread.remote_thread_id)?;

    let mut rendered = Vec::new();
    let mut has_remote = false;
    for message in &api_thread.messages {
        let extracted = gmail_api::extract_body(message);
        let body_html = match (&extracted.html, &extracted.text_plain) {
            (Some(html), _) => render::sanitize(html),
            (None, Some(text)) => render::text_to_html(text),
            (None, None) => no_content(),
        };
        has_remote |= render::has_remote_content(&body_html);

        if let Some(row) = rows.iter().find(|m| m.remote_msg_id == message.id) {
            let _ = store.set_body(
                row.id,
                extracted.text_plain.as_deref(),
                extracted.html.as_deref(),
                Some(&body_html),
            );
        }
        rendered.push(RenderMessage {
            from: message.header("From").unwrap_or("").to_string(),
            date: message.internal_date_ms().map(msg_date_from_ms).unwrap_or_default(),
            body_html,
        });
    }
    Ok((rendered, has_remote))
}

// --- small widget/helpers ---

fn thread_row_widget(thread: &ThreadRow) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    row.add_css_class("thread-row");
    if thread.unread {
        row.add_css_class("unread");
    }

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let sender = gtk::Label::builder()
        .label(display_sender(&thread.sender))
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    sender.add_css_class("thread-sender");
    let date = gtk::Label::builder()
        .label(thread_date(&thread.last_date))
        .build();
    date.add_css_class("thread-date");
    top.append(&sender);
    top.append(&date);

    let subject = gtk::Label::builder()
        .label(&thread.subject)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    subject.add_css_class("thread-subject");

    let snippet = gtk::Label::builder()
        .label(&thread.snippet)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    snippet.add_css_class("thread-snippet");

    row.append(&top);
    row.append(&subject);
    row.append(&snippet);
    row
}

fn sidebar_action_button(label: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.set_tooltip_text(Some(tooltip));
    button.add_css_class("sidebar-action-button");
    button
}

fn clear(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

/// Extracts a display name from a `From` header: `"Jane Doe <j@x.com>"` -> `Jane
/// Doe`; a bare address is returned as-is.
fn display_sender(from: &str) -> String {
    let from = from.trim();
    if let Some(idx) = from.find('<') {
        let name = from[..idx].trim().trim_matches('"').trim();
        if !name.is_empty() {
            return name.to_string();
        }
        return from[idx + 1..].trim_end_matches('>').trim().to_string();
    }
    from.to_string()
}

fn parse_local(iso: &str) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Local))
}

fn thread_date(iso: &str) -> String {
    parse_local(iso)
        .map(|dt| dt.format("%b %-d").to_string())
        .unwrap_or_default()
}

fn msg_date(iso: &str) -> String {
    parse_local(iso)
        .map(|dt| dt.format("%b %-d, %Y · %-I:%M %p").to_string())
        .unwrap_or_default()
}

fn msg_date_from_ms(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|dt| msg_date(&dt.to_rfc3339()))
        .unwrap_or_default()
}

fn first_line(error: &str) -> &str {
    error.lines().next().unwrap_or(error)
}

fn no_content() -> String {
    "<p style=\"color:#888;padding:20px\">(no content)</p>".to_string()
}

fn loading_page() -> String {
    "<html><body style=\"font-family:sans-serif;color:#888;padding:24px\">Loading…</body></html>"
        .to_string()
}

fn error_page(message: &str) -> String {
    format!(
        "<html><body style=\"font-family:sans-serif;color:#b00020;padding:24px\">\
         Couldn't load this conversation:<br><br>{}</body></html>",
        render::text_to_html(message)
    )
}
