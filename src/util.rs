//! Small cross-platform helpers.

/// Opens a URL in the user's default browser: `open` on macOS, `xdg-open`
/// elsewhere (Linux/BSD). One of Mailix's few per-OS seams — used for the
/// OAuth consent redirect and for links clicked inside a rendered message.
pub fn open_in_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    const OPENER: &str = "open";
    #[cfg(not(target_os = "macos"))]
    const OPENER: &str = "xdg-open";
    std::process::Command::new(OPENER).arg(url).spawn().map(|_| ())
}
