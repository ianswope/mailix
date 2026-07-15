use serde::Deserialize;

#[derive(Deserialize, Clone)]
pub struct GoogleConfig {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Deserialize, Default)]
pub struct Config {
    pub google: Option<GoogleConfig>,
}

impl Config {
    /// Loads `~/.config/mailix/config.toml`. A missing file or an absent
    /// `[google]` section both just mean "Google isn't configured yet" — not
    /// an error, since that's the normal state until a user follows the
    /// README's OAuth client setup steps. iCloud/IMAP accounts need nothing
    /// here; only Google requires a bring-your-own OAuth client.
    pub fn load() -> Config {
        let path = gtk::glib::user_config_dir()
            .join("mailix")
            .join("config.toml");
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return Config::default();
        };
        toml::from_str(&contents).unwrap_or_else(|e| {
            eprintln!("mailix: failed to parse {}: {e}", path.display());
            Config::default()
        })
    }
}
