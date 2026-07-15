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
        Config::from_toml(&contents).unwrap_or_else(|e| {
            eprintln!("mailix: failed to parse {}: {e}", path.display());
            Config::default()
        })
    }

    /// Parses config TOML. Split out from `load` so the parsing rules (an empty
    /// file or an absent `[google]` section yields `google: None`; malformed
    /// TOML is an error the caller downgrades to defaults) are unit-testable
    /// without touching the filesystem.
    pub(crate) fn from_toml(contents: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_google_section() {
        let cfg =
            Config::from_toml("[google]\nclient_id = \"id123\"\nclient_secret = \"secret456\"\n")
                .unwrap();
        let google = cfg.google.expect("google section present");
        assert_eq!(google.client_id, "id123");
        assert_eq!(google.client_secret, "secret456");
    }

    #[test]
    fn empty_config_has_no_google() {
        assert!(Config::from_toml("").unwrap().google.is_none());
    }

    #[test]
    fn malformed_toml_is_an_error() {
        assert!(Config::from_toml("not valid").is_err());
    }
}
