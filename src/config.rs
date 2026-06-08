//! User configuration — region, locale, timezone, subscriptions.
//!
//! Stored at `~/.config/animesh/config.toml` (XDG-compliant via the
//! `directories` crate). Schema-versioned: every breaking change
//! bumps [`Config::SCHEMA_VERSION`] and adds a migration in
//! [`Config::migrate`]. v0.5 ships version 1.
//!
//! ## Why TOML
//!
//! TOML is the right shape for a small user-facing config: comments
//! survive round-trip-ish, the syntax is unambiguous about strings vs
//! identifiers, and humans edit it. JSON requires quoting every key;
//! YAML's indentation rules are landmines.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Top-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    /// Schema version of this file. Increment when the shape changes
    /// in an incompatible way; [`Config::migrate`] handles the bump.
    #[serde(default = "Config::default_schema_version")]
    pub schema_version: u32,
    /// ISO 3166-1 alpha-2 region code. Drives TMDB watch-provider
    /// queries — "IN", "US", "GB", "JP".
    pub region: String,
    /// BCP-47 locale tag. "en-US", "ja-JP". Drives display formatting.
    pub locale: String,
    /// IANA timezone name. "Asia/Kolkata", "America/Los_Angeles".
    pub timezone: String,
    /// What the user pays for.
    #[serde(default)]
    pub subscriptions: Subscriptions,
}

/// Streamer subscriptions by media kind.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Subscriptions {
    /// Video streamers — "Netflix", "Crunchyroll", "Apple TV+".
    /// Match these case-insensitively against source `streaming_links`
    /// `site` fields.
    #[serde(default)]
    pub video: Vec<String>,
    /// Audio streamers — "Spotify", "Apple Music", "YouTube Music".
    #[serde(default)]
    pub audio: Vec<String>,
}

impl Config {
    /// Current schema version. Bump when the file shape changes.
    pub const SCHEMA_VERSION: u32 = 1;

    fn default_schema_version() -> u32 {
        Self::SCHEMA_VERSION
    }

    /// Sensible-ish defaults for a fresh install. Used when no
    /// config file exists yet. Region defaults to "US" so notifier
    /// + TMDB don't fail closed; the user is expected to override.
    pub fn defaults() -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            region: "US".into(),
            locale: "en-US".into(),
            timezone: "UTC".into(),
            subscriptions: Subscriptions::default(),
        }
    }

    /// Load from disk; on first read, return defaults without
    /// writing. Callers that want the file materialized should call
    /// [`Config::save`] explicitly.
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::defaults());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read config from {path:?}"))?;
        Self::from_toml(&text)
    }

    /// Parse from a TOML string. Runs migrations from any older
    /// schema version to the current one.
    pub fn from_toml(text: &str) -> Result<Self> {
        let mut cfg: Self = toml::from_str(text).context("parse config TOML")?;
        if cfg.schema_version > Self::SCHEMA_VERSION {
            bail!(
                "config schema_version is {} but binary only knows up to {}; \
                 upgrade animesh or downgrade the config",
                cfg.schema_version,
                Self::SCHEMA_VERSION
            );
        }
        cfg.migrate()?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Persist to disk. Creates parent dirs as needed. Atomic via
    /// write-to-tmp-then-rename so a crash mid-write doesn't leave a
    /// corrupt config.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create config dir {parent:?}"))?;
        }
        let text = toml::to_string_pretty(self).context("serialize config")?;
        // Write to a sibling tmp file then rename — atomic on POSIX.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("write {tmp:?}"))?;
        std::fs::rename(&tmp, path).with_context(|| format!("rename {tmp:?} -> {path:?}"))?;
        Ok(())
    }

    /// Default user config path — `~/.config/animesh/config.toml` on
    /// Linux/macOS via the `directories` crate.
    pub fn default_path() -> Result<PathBuf> {
        let dirs = directories::BaseDirs::new()
            .ok_or_else(|| anyhow!("HOME / config dir not found"))?;
        Ok(dirs.config_dir().join("animesh").join("config.toml"))
    }

    fn validate(&self) -> Result<()> {
        if self.region.len() != 2 || !self.region.chars().all(|c| c.is_ascii_uppercase()) {
            bail!(
                "config.region must be an ISO 3166-1 alpha-2 code (e.g. 'US'), got {:?}",
                self.region
            );
        }
        if self.locale.is_empty() {
            bail!("config.locale must not be empty");
        }
        if self.timezone.is_empty() {
            bail!("config.timezone must not be empty");
        }
        Ok(())
    }

    fn migrate(&mut self) -> Result<()> {
        // No migrations needed for v0.5 — schema is at v1 and stays
        // there. Future bumps land here as `if self.schema_version < N { ... self.schema_version = N; }`.
        if self.schema_version == 0 {
            // Treat missing version as v1 (tolerate hand-written
            // initial configs).
            self.schema_version = 1;
        }
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample() -> Config {
        Config {
            schema_version: 1,
            region: "IN".into(),
            locale: "en-IN".into(),
            timezone: "Asia/Kolkata".into(),
            subscriptions: Subscriptions {
                video: vec!["Netflix".into(), "Crunchyroll".into()],
                audio: vec!["Spotify".into()],
            },
        }
    }

    #[test]
    fn defaults_passes_validation() {
        let cfg = Config::defaults();
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.schema_version, Config::SCHEMA_VERSION);
    }

    #[test]
    fn load_or_default_returns_defaults_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = Config::load_or_default(&path).unwrap();
        assert_eq!(cfg, Config::defaults());
        // load_or_default must NOT create the file.
        assert!(!path.exists());
    }

    #[test]
    fn save_then_load_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = sample();
        cfg.save(&path).unwrap();
        let loaded = Config::load_or_default(&path).unwrap();
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn save_creates_missing_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("a").join("b").join("config.toml");
        sample().save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_is_atomic_via_tmp_rename() {
        // The tmp file should not survive a successful save.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let tmp = path.with_extension("toml.tmp");
        sample().save(&path).unwrap();
        assert!(path.exists());
        assert!(!tmp.exists(), "tmp must be renamed away");
    }

    #[test]
    fn from_toml_parses_minimal_config_with_defaults_for_missing_fields() {
        let text = r#"
region = "JP"
locale = "ja-JP"
timezone = "Asia/Tokyo"
"#;
        let cfg = Config::from_toml(text).unwrap();
        assert_eq!(cfg.region, "JP");
        // No subscriptions section → defaults to empty.
        assert!(cfg.subscriptions.video.is_empty());
        assert!(cfg.subscriptions.audio.is_empty());
        // Missing schema_version → defaults to SCHEMA_VERSION.
        assert_eq!(cfg.schema_version, Config::SCHEMA_VERSION);
    }

    #[test]
    fn from_toml_rejects_invalid_region() {
        let text = r#"
region = "USA"
locale = "en-US"
timezone = "UTC"
"#;
        let err = Config::from_toml(text).unwrap_err();
        assert!(format!("{err}").contains("ISO 3166"));
    }

    #[test]
    fn from_toml_rejects_lowercase_region() {
        let text = r#"
region = "us"
locale = "en-US"
timezone = "UTC"
"#;
        let err = Config::from_toml(text).unwrap_err();
        assert!(format!("{err}").contains("ISO 3166"));
    }

    #[test]
    fn from_toml_rejects_empty_locale() {
        let text = r#"
region = "US"
locale = ""
timezone = "UTC"
"#;
        let err = Config::from_toml(text).unwrap_err();
        assert!(format!("{err}").contains("locale"));
    }

    #[test]
    fn from_toml_refuses_future_schema_version() {
        let text = r#"
schema_version = 99
region = "US"
locale = "en-US"
timezone = "UTC"
"#;
        let err = Config::from_toml(text).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("99"), "got: {msg}");
        assert!(msg.contains("upgrade animesh"), "remediation hint: {msg}");
    }

    #[test]
    fn migrate_treats_missing_schema_version_as_v1() {
        // Hand-written config that forgot the schema_version line.
        let text = r#"
schema_version = 0
region = "US"
locale = "en-US"
timezone = "UTC"
"#;
        let cfg = Config::from_toml(text).unwrap();
        assert_eq!(cfg.schema_version, 1);
    }

    #[test]
    fn subscriptions_preserve_case_in_serialized_form() {
        let cfg = sample();
        let text = toml::to_string(&cfg).unwrap();
        assert!(text.contains("Netflix"));
        assert!(text.contains("Crunchyroll"));
    }
}
