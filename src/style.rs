use gtk::gdk;

// Mailix's own small CSS. Everything is expressed through libadwaita named
// colors (`@accent_bg_color`, `@window_fg_color`, `@borders`, …) so the
// Omarchy overrides loaded below recolor it for free — the same approach as
// calix. Layout/spacing lives here; color comes from the theme.
const CSS: &str = "
.mailbox-sidebar {
    background-color: @sidebar_bg_color;
}

.sidebar-section-label {
    font-size: 0.8em;
    font-weight: bold;
    opacity: 0.6;
    padding: 10px 12px 4px 12px;
}

.sidebar-action-button {
    min-height: 30px;
    padding-left: 8px;
    padding-right: 8px;
}

/* Conversation rows in the thread list. */
.thread-row {
    padding: 8px 12px;
    border-bottom: 1px solid alpha(@borders, 0.6);
}

.thread-row .thread-sender {
    font-weight: bold;
}

.thread-row.unread .thread-sender,
.thread-row.unread .thread-subject {
    font-weight: bold;
}

.thread-row .thread-snippet {
    opacity: 0.65;
    font-size: 0.9em;
}

.thread-row .thread-date {
    opacity: 0.6;
    font-size: 0.85em;
}

.unread-dot {
    background-color: @accent_bg_color;
    border-radius: 999px;
    min-width: 8px;
    min-height: 8px;
}

/* The header strip above a rendered message body. */
.message-header {
    padding: 12px 16px;
    border-bottom: 1px solid @borders;
}

.message-header .message-subject {
    font-size: 1.15em;
    font-weight: bold;
}

/* Banner offering to load blocked remote images. */
.remote-content-banner {
    background-color: alpha(@warning_bg_color, 0.15);
    border-bottom: 1px solid @borders;
    padding: 6px 12px;
}
";

pub fn load() {
    let display = gdk::Display::default().expect("a display is available");

    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // If Omarchy is present, recolor libadwaita from the active theme. The
    // overrides load at USER priority so they win over libadwaita's own
    // (theme-priority) color definitions, and force the matching color scheme
    // so symbolic icons and dark-aware widgets line up with the palette. The
    // layout CSS above keeps referencing the same color names either way.
    if let Some(overrides) = crate::omarchy::theme_overrides() {
        let color_provider = gtk::CssProvider::new();
        color_provider.load_from_string(&overrides.css);
        gtk::style_context_add_provider_for_display(
            &display,
            &color_provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
        adw::StyleManager::default().set_color_scheme(if overrides.dark {
            adw::ColorScheme::ForceDark
        } else {
            adw::ColorScheme::ForceLight
        });
    }
}
