//! User settings, stored as TOML in the XDG config directory.
//!
//! Settings never hold secrets: the session token lives in the OS keyring (see
//! [`crate::tokens`]), not in this file.

use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Why settings could not be read, written or used.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// The file could not be read or written.
    #[error("settings file: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not valid TOML for these settings.
    #[error("settings file is malformed: {0}")]
    Parse(#[from] toml::de::Error),
    /// The settings could not be serialized.
    #[error("could not write settings: {0}")]
    Serialize(#[from] toml::ser::Error),
    /// The configured server address cannot be used.
    #[error("invalid server address: {0}")]
    InvalidServerUrl(String),
}

/// What the user can configure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Address of a pulldeck server, e.g. `https://pulldeck.example.com`. Without one the app
    /// runs standalone and polls GitHub itself.
    pub server_url: Option<String>,
    /// Tell other users which PR you are looking at. Off unless the user turns it on.
    pub presence: bool,
    /// Show a desktop notification when another user sends you something. On unless turned off.
    pub notifications: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server_url: None,
            presence: false,
            notifications: true,
        }
    }
}

impl Settings {
    /// `$XDG_CONFIG_HOME/pulldeck/config.toml`, falling back to `~/.config/pulldeck/config.toml`.
    pub fn default_path() -> Option<PathBuf> {
        config_path(
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        )
    }

    /// Reads settings from `path`. A missing file is not an error: it yields the defaults.
    pub fn load_from(path: &Path) -> Result<Self, SettingsError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes settings to `path`, creating its directory. The write goes to a temporary file
    /// first and is renamed into place, so a crash never leaves a half-written file.
    pub fn save_to(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("toml.tmp");
        std::fs::write(&temp, toml::to_string_pretty(self)?)?;
        std::fs::rename(&temp, path)?;
        Ok(())
    }

    /// The validated server address, if one is configured.
    pub fn server(&self) -> Result<Option<ServerUrl>, SettingsError> {
        self.server_url.as_deref().map(ServerUrl::parse).transpose()
    }
}

fn config_path(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = match xdg.filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(home.filter(|v| !v.is_empty())?).join(".config"),
    };
    Some(base.join("pulldeck").join("config.toml"))
}

/// A validated server address.
///
/// Plain `http` is accepted only for loopback hosts, so a session token is never sent in clear
/// text across a network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerUrl {
    base: String,
}

impl ServerUrl {
    /// Validates and normalizes an address (no trailing slash).
    pub fn parse(input: &str) -> Result<Self, SettingsError> {
        let invalid = |why: &str| SettingsError::InvalidServerUrl(format!("{input:?}: {why}"));
        let input = input.trim();

        let (scheme, rest) = input
            .split_once("://")
            .ok_or_else(|| invalid("expected https://host or http://localhost"))?;
        if scheme != "https" && scheme != "http" {
            return Err(invalid("scheme must be https (or http for localhost)"));
        }
        if rest.contains(['?', '#']) {
            return Err(invalid("must not contain a query or fragment"));
        }

        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.contains('@') {
            return Err(invalid("must not contain credentials"));
        }
        let (host, port) = split_host_port(authority).ok_or_else(|| invalid("bad host or port"))?;
        if host.is_empty() {
            return Err(invalid("missing host"));
        }
        if scheme == "http" && !is_loopback(host) {
            return Err(invalid(
                "plain http is only allowed for localhost; use https",
            ));
        }

        let port = port.map(|p| format!(":{p}")).unwrap_or_default();
        let path = path.trim_end_matches('/');
        Ok(Self {
            base: format!("{scheme}://{host}{port}{path}"),
        })
    }

    /// The address with a path appended, e.g. `url("/auth/device")`.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// The websocket endpoint: `wss://` for https, `ws://` for loopback http.
    pub fn websocket(&self) -> String {
        let base = match self.base.strip_prefix("https://") {
            Some(rest) => format!("wss://{rest}"),
            None => format!("ws://{}", &self.base["http://".len()..]),
        };
        format!("{base}/ws")
    }
}

/// Splits `host[:port]`, understanding `[::1]:8080`. `None` if the port is not a number.
fn split_host_port(authority: &str) -> Option<(&str, Option<u16>)> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let host_len = host.len() + 2; // keep the brackets in the host
        let port = after.strip_prefix(':');
        if !after.is_empty() && port.is_none() {
            return None;
        }
        (&authority[..host_len], port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let port = match port {
        Some(p) => Some(p.parse().ok()?),
        None => None,
    };
    Some((host, port))
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<ServerUrl, SettingsError> {
        ServerUrl::parse(s)
    }

    #[test]
    fn defaults_are_standalone_with_presence_off() {
        let s = Settings::default();
        assert_eq!(s.server_url, None);
        assert!(!s.presence);
        assert!(s.notifications);
        assert_eq!(s.server().unwrap(), None);
    }

    #[test]
    fn a_missing_file_yields_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = Settings::load_from(&dir.path().join("nope.toml")).unwrap();
        assert_eq!(loaded, Settings::default());
    }

    #[test]
    fn settings_round_trip_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.toml");
        let settings = Settings {
            server_url: Some("https://deck.example.com".into()),
            presence: true,
            notifications: false,
        };
        settings.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path).unwrap(), settings);
        assert!(
            !path.with_extension("toml.tmp").exists(),
            "temp file is renamed away"
        );
    }

    #[test]
    fn partial_and_future_files_still_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "presence = true\nsome_future_key = 1\n").unwrap();
        let s = Settings::load_from(&path).unwrap();
        assert!(s.presence);
        assert_eq!(s.server_url, None);
    }

    #[test]
    fn a_malformed_file_is_an_error_not_silently_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "presence = [not toml").unwrap();
        assert!(matches!(
            Settings::load_from(&path),
            Err(SettingsError::Parse(_))
        ));
    }

    #[test]
    fn config_path_prefers_xdg_then_home() {
        let p = |xdg: Option<&str>, home: Option<&str>| {
            config_path(xdg.map(OsString::from), home.map(OsString::from))
        };
        assert_eq!(
            p(Some("/x"), Some("/h")),
            Some("/x/pulldeck/config.toml".into())
        );
        assert_eq!(
            p(None, Some("/h")),
            Some("/h/.config/pulldeck/config.toml".into())
        );
        assert_eq!(
            p(Some(""), Some("/h")),
            Some("/h/.config/pulldeck/config.toml".into())
        );
        assert_eq!(p(None, None), None);
        assert_eq!(p(None, Some("")), None);
    }

    #[test]
    fn https_addresses_are_normalized() {
        assert_eq!(
            parse("https://deck.example.com").unwrap().url("/x"),
            "https://deck.example.com/x"
        );
        assert_eq!(
            parse("  https://deck.example.com/  ").unwrap().url(""),
            "https://deck.example.com"
        );
        assert_eq!(
            parse("https://deck.example.com:8443/pulldeck/")
                .unwrap()
                .url("/auth/device"),
            "https://deck.example.com:8443/pulldeck/auth/device"
        );
    }

    #[test]
    fn websocket_urls_follow_the_scheme() {
        assert_eq!(
            parse("https://deck.example.com").unwrap().websocket(),
            "wss://deck.example.com/ws"
        );
        assert_eq!(
            parse("https://h:8443/p").unwrap().websocket(),
            "wss://h:8443/p/ws"
        );
        assert_eq!(
            parse("http://localhost:3000").unwrap().websocket(),
            "ws://localhost:3000/ws"
        );
    }

    #[test]
    fn plain_http_is_only_for_loopback() {
        for ok in [
            "http://localhost",
            "http://localhost:8080",
            "http://127.0.0.1:1",
            "http://[::1]:9",
        ] {
            assert!(parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://deck.example.com",
            "http://10.0.0.5",
            "http://localhost.evil.com",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn unusable_addresses_are_rejected() {
        for bad in [
            "",
            "deck.example.com",
            "ftp://deck.example.com",
            "https://",
            "https://user:pw@deck.example.com",
            "https://deck.example.com?x=1",
            "https://deck.example.com/#frag",
            "https://deck.example.com:notaport",
            "https://deck.example.com:99999",
            "https://[::1",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }
}
