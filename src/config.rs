//! The pgterm profile file: ~/.config/pgterm/config.toml (XDG-aware on every
//! OS — the same convention pgbot itself uses, never Library/Application
//! Support on macOS).
//!
//! The file stores environment-variable NAMES, never connection strings. A
//! resolved DSN must never travel through this module.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _};
use serde::{Deserialize, Serialize};

pub const DEFAULT_INTERVAL_SECONDS: u64 = 60;
pub const DEFAULT_MAX_CONCURRENT_CHECKS: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub interval_seconds: u64,
    pub max_concurrent_checks: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            interval_seconds: DEFAULT_INTERVAL_SECONDS,
            max_concurrent_checks: DEFAULT_MAX_CONCURRENT_CHECKS,
        }
    }
}

/// Which environment a database belongs to. Drives the sidebar badge and,
/// in later slices, the extra confirmation before writes on PROD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Prod,
    Staging,
    Dev,
    Local,
}

impl Stage {
    pub const ALL: [Stage; 4] = [Stage::Prod, Stage::Staging, Stage::Dev, Stage::Local];

    pub fn label(self) -> &'static str {
        match self {
            Stage::Prod => "PROD",
            Stage::Staging => "STAGING",
            Stage::Dev => "DEV",
            Stage::Local => "LOCAL",
        }
    }

    pub fn parse(s: &str) -> Option<Stage> {
        match s.trim().to_ascii_lowercase().as_str() {
            "prod" => Some(Stage::Prod),
            "staging" => Some(Stage::Staging),
            "dev" => Some(Stage::Dev),
            "local" => Some(Stage::Local),
            _ => None,
        }
    }

    /// Best guess from a name, in a fixed order so "prod" beats "dev" in
    /// "dev-prod-mirror". An explicit `stage =` in config always wins.
    pub fn infer(name: &str) -> Option<Stage> {
        let n = name.to_ascii_lowercase();
        if n.contains("prod") {
            Some(Stage::Prod)
        } else if n.contains("stag") {
            Some(Stage::Staging)
        } else if n.contains("local") {
            Some(Stage::Local)
        } else if n.contains("dev") {
            Some(Stage::Dev)
        } else {
            None
        }
    }
}

/// Presentation switches. Nothing here changes what pgterm does to a database.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiSettings {
    /// Two rows per database in the sidebar: the second is a dim detail line.
    pub sidebar_detail: bool,
    /// Ring the terminal bell with a toast when an unselected database turns
    /// critical or unavailable.
    pub bell: bool,
    /// Ask the terminal for a hand pointer over clickable things (OSC 22).
    /// Terminals that do not implement it ignore the sequence.
    pub pointer: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        UiSettings {
            sidebar_detail: true,
            bell: false,
            pointer: true,
        }
    }
}

/// The annotated defaults, printed by `pgterm --default-config`.
pub const DEFAULT_CONFIG_TEXT: &str = "\
# pgterm configuration \u{2014} ~/.config/pgterm/config.toml
# Stores environment-variable NAMES, never connection strings.
version = 1

[settings]
# Seconds between background health checks of every database.
interval_seconds = 60
# How many pgbot checks may run at once.
max_concurrent_checks = 3

[ui]
# Two rows per database in the sidebar (the second is a dim detail line).
sidebar_detail = true
# Terminal bell when a database you are not looking at turns critical.
bell = false
# Ask the terminal for a hand pointer over clickable things (OSC 22).
# Works in Ghostty, kitty, WezTerm, foot and xterm; others ignore it.
pointer = true

# One block per database:
# [[databases]]
# name = \"production\"
# env = \"PROD_DATABASE_URL\"   # the variable holding the connection string
# stage = \"prod\"              # prod | staging | dev | local \u{2014} badge; inferred from the name when absent
# pgrun_project = \"acme-api\"  # show this pgrun project's branches for the database
# writes = false               # true lets the SQL tab write (a PROD badge still asks first)
";

/// One monitored database: a friendly name and the environment variable that
/// holds its connection string. The variable's VALUE is resolved in memory at
/// spawn time and never persisted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DatabaseProfile {
    pub name: String,
    pub env: String,
    /// The environment badge. Absent means "infer it from the name".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<Stage>,
    /// The pgrun project whose branches belong to this database. Absent means
    /// the Branches section explains how to set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pgrun_project: Option<String>,
    /// May the SQL tab write? Off by default: every statement runs in a
    /// READ ONLY transaction until this is set.
    #[serde(default, skip_serializing_if = "is_false")]
    pub writes: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl DatabaseProfile {
    /// The badge to show: the configured stage, else one inferred from the name.
    pub fn badge(&self) -> Option<Stage> {
        self.stage.or_else(|| Stage::infer(&self.name))
    }

    /// Read-only unless the profile opted in; a prod badge additionally asks
    /// before each write.
    pub fn write_policy(&self) -> crate::db::WritePolicy {
        use crate::db::WritePolicy;
        match (self.writes, self.badge()) {
            (false, _) => WritePolicy::ReadOnly,
            (true, Some(Stage::Prod)) => WritePolicy::ConfirmWrites,
            (true, _) => WritePolicy::Writes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TerminalConfig {
    pub version: u32,
    pub settings: Settings,
    pub ui: UiSettings,
    #[serde(rename = "databases")]
    pub databases: Vec<DatabaseProfile>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        TerminalConfig {
            version: 1,
            settings: Settings::default(),
            ui: UiSettings::default(),
            databases: Vec::new(),
        }
    }
}

/// Where the profile file lives: $PGTERM_CONFIG (tests, unusual setups) →
/// $XDG_CONFIG_HOME/pgterm/config.toml → ~/.config/pgterm/config.toml.
pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("PGTERM_CONFIG") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Path::new(&xdg).join("pgterm").join("config.toml");
        }
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".config").join("pgterm").join("config.toml")
}

impl TerminalConfig {
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(TerminalConfig::default())
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let cfg: TerminalConfig =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&config_path())
    }

    /// Atomic write: temp file in the same directory, then rename. 0600 on
    /// unix — the file holds no secrets, but it maps out infrastructure.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let text = toml::to_string_pretty(self).context("encoding terminal.toml")?;
        let tmp = dir.join(".terminal.toml.tmp");
        std::fs::write(&tmp, &text).with_context(|| format!("writing {}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path)
            .with_context(|| format!("moving into place at {}", path.display()))?;
        Ok(())
    }

    /// Validates and appends a profile. Names are what tabs display: short,
    /// shell-friendly, unique.
    pub fn add(&mut self, name: &str, env: &str) -> anyhow::Result<()> {
        self.add_with_stage(name, env, None)
    }

    /// `add`, with the environment badge the caller chose (None = infer).
    pub fn add_with_stage(
        &mut self,
        name: &str,
        env: &str,
        stage: Option<Stage>,
    ) -> anyhow::Result<()> {
        validate_name(name)?;
        if env.is_empty() {
            bail!("environment variable name is empty");
        }
        if looks_like_connection_string(env) {
            bail!(
                "that looks like a connection string — pgterm stores variable NAMES, never URLs. \
                 Export it first (export MY_DB_URL='postgresql://...'), then reference MY_DB_URL"
            );
        }
        if !env.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            // Never echo the rejected input: it may be a mistyped DSN.
            bail!("not a valid environment variable name (letters, digits and '_' only)");
        }
        if self.databases.iter().any(|d| d.name == name) {
            bail!("database \"{name}\" already exists (pgterm list)");
        }
        self.databases.push(DatabaseProfile {
            name: name.to_string(),
            env: env.to_string(),
            stage,
            pgrun_project: None,
            writes: false,
        });
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> anyhow::Result<DatabaseProfile> {
        match self.databases.iter().position(|d| d.name == name) {
            Some(i) => Ok(self.databases.remove(i)),
            None => bail!("no database named \"{name}\" (pgterm list)"),
        }
    }
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        bail!("database name is empty");
    }
    if name.len() > 64 {
        bail!("database name is longer than 64 characters");
    }
    if looks_like_connection_string(name) {
        bail!(
            "that looks like a connection string — the name is just a label (prod, staging). \
             The URL goes into an environment variable referenced by --env"
        );
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        // Never echo the rejected input: it may be a mistyped DSN.
        bail!("database name may only contain letters, digits, '.', '_' and '-'");
    }
    Ok(())
}

/// A URL or keyword-DSN shape where a name was expected. Deliberately broad:
/// anything with '://', '=', or whitespace cannot be a env-var name anyway,
/// and mistyping a DSN here is the common mistake worth a real explanation.
fn looks_like_connection_string(s: &str) -> bool {
    s.contains("://") || s.contains('=') || s.contains('@')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpfile(dir: &tempdir::TempDir) -> PathBuf {
        dir.path().join("terminal.toml")
    }

    // No tempdir crate in the dependency list — use std temp dirs.
    mod tempdir {
        use std::path::{Path, PathBuf};
        pub struct TempDir(PathBuf);
        impl TempDir {
            pub fn new(tag: &str) -> std::io::Result<TempDir> {
                let p =
                    std::env::temp_dir().join(format!("pgterm-test-{tag}-{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&p);
                std::fs::create_dir_all(&p)?;
                Ok(TempDir(p))
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn missing_file_loads_default() {
        let cfg = TerminalConfig::load_from(Path::new("/nonexistent/terminal.toml")).unwrap();
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.settings.interval_seconds, 60);
        assert_eq!(cfg.settings.max_concurrent_checks, 3);
        assert!(cfg.databases.is_empty());
    }

    #[test]
    fn round_trip_preserves_profiles() {
        let dir = tempdir::TempDir::new("roundtrip").unwrap();
        let path = tmpfile(&dir);
        let mut cfg = TerminalConfig::default();
        cfg.add("production", "PROD_DATABASE_URL").unwrap();
        cfg.add("staging", "STAGING_DATABASE_URL").unwrap();
        cfg.save_to(&path).unwrap();
        let back = TerminalConfig::load_from(&path).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn parses_the_spec_example_verbatim() {
        let text = r#"
version = 1

[settings]
interval_seconds = 60
max_concurrent_checks = 3

[[databases]]
name = "production"
env = "PROD_DATABASE_URL"

[[databases]]
name = "staging"
env = "STAGING_DATABASE_URL"
"#;
        let cfg: TerminalConfig = toml::from_str(text).unwrap();
        assert_eq!(cfg.databases.len(), 2);
        assert_eq!(cfg.databases[0].name, "production");
        assert_eq!(cfg.databases[0].env, "PROD_DATABASE_URL");
    }

    #[test]
    fn duplicate_name_is_rejected() {
        let mut cfg = TerminalConfig::default();
        cfg.add("prod", "A_URL").unwrap();
        let err = cfg.add("prod", "B_URL").unwrap_err().to_string();
        assert!(err.contains("already exists"), "{err}");
        assert_eq!(cfg.databases.len(), 1);
    }

    #[test]
    fn bad_names_and_envs_are_rejected() {
        let mut cfg = TerminalConfig::default();
        assert!(cfg.add("", "X").is_err());
        assert!(cfg.add("has space", "X").is_err());
        assert!(cfg.add("semi;colon", "X").is_err());
        assert!(cfg.add(&"x".repeat(65), "X").is_err());
        assert!(cfg.add("ok", "").is_err());
        assert!(cfg.add("ok", "NOT VALID").is_err());
        assert!(cfg.add("ok", "$(whoami)").is_err());
        assert!(cfg.databases.is_empty());
    }

    #[test]
    fn remove_returns_profile_and_errors_on_missing() {
        let mut cfg = TerminalConfig::default();
        cfg.add("staging", "S_URL").unwrap();
        let gone = cfg.remove("staging").unwrap();
        assert_eq!(gone.env, "S_URL");
        assert!(cfg.remove("staging").is_err());
    }

    #[test]
    fn saved_file_never_contains_a_connection_string() {
        // Even with a DSN sitting in the process env, only the NAME is stored.
        let dir = tempdir::TempDir::new("nosecret").unwrap();
        let path = tmpfile(&dir);
        std::env::set_var(
            "PGBOT_TEST_SECRET_URL",
            "postgres://alex:super-secret@host/db",
        );
        let mut cfg = TerminalConfig::default();
        cfg.add("prod", "PGBOT_TEST_SECRET_URL").unwrap();
        cfg.save_to(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("postgres://"), "config leaked a DSN: {text}");
        assert!(!text.contains("super-secret"), "config leaked a password");
        assert!(text.contains("env = \"PGBOT_TEST_SECRET_URL\""));
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir::TempDir::new("perms").unwrap();
        let path = tmpfile(&dir);
        TerminalConfig::default().save_to(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode was {mode:o}");
    }

    #[test]
    fn unknown_keys_are_tolerated() {
        let text = "version = 1\nfuture_knob = true\n[settings]\ninterval_seconds = 30\n";
        let cfg: TerminalConfig = toml::from_str(text).unwrap();
        assert_eq!(cfg.settings.interval_seconds, 30);
    }

    #[test]
    fn connection_strings_get_guidance_and_are_never_echoed() {
        let mut cfg = TerminalConfig::default();
        // URL typed where the env-var NAME belongs — the common mistake.
        let err = cfg
            .add("prod", "postgres://alex:hunter2@db.example/app")
            .unwrap_err()
            .to_string();
        assert!(err.contains("connection string"), "{err}");
        assert!(err.contains("export"), "tells the user the fix: {err}");
        assert!(!err.contains("hunter2"), "password echoed: {err}");
        assert!(!err.contains("db.example"), "input echoed: {err}");

        // Keyword DSN as env value.
        let err = cfg
            .add("prod", "host=h password=sekret")
            .unwrap_err()
            .to_string();
        assert!(err.contains("connection string"), "{err}");
        assert!(!err.contains("sekret"), "password echoed: {err}");

        // URL typed as the database NAME.
        let err = cfg
            .add("postgres://alex:hunter2@h/db", "X_URL")
            .unwrap_err()
            .to_string();
        assert!(err.contains("connection string"), "{err}");
        assert!(!err.contains("hunter2"), "password echoed: {err}");

        // Plain invalid input is refused without being echoed either.
        let err = cfg.add("prod", "NOT VALID pw=1").unwrap_err().to_string();
        assert!(!err.contains("pw=1"), "{err}");
        assert!(cfg.databases.is_empty());
    }

    #[test]
    fn stage_parses_and_labels() {
        assert_eq!(Stage::parse("PROD"), Some(Stage::Prod));
        assert_eq!(Stage::parse("staging"), Some(Stage::Staging));
        assert_eq!(Stage::parse("production"), None);
        assert_eq!(Stage::Prod.label(), "PROD");
        assert_eq!(Stage::Local.label(), "LOCAL");
    }

    #[test]
    fn stage_is_inferred_from_the_name_explicit_wins() {
        assert_eq!(Stage::infer("production"), Some(Stage::Prod));
        assert_eq!(Stage::infer("eu-prod-2"), Some(Stage::Prod));
        assert_eq!(Stage::infer("staging"), Some(Stage::Staging));
        assert_eq!(Stage::infer("stage"), Some(Stage::Staging));
        assert_eq!(Stage::infer("localhost"), Some(Stage::Local));
        assert_eq!(Stage::infer("dev-box"), Some(Stage::Dev));
        assert_eq!(Stage::infer("analytics"), None);
        let p = DatabaseProfile {
            name: "production".into(),
            env: "X".into(),
            stage: Some(Stage::Dev),
            pgrun_project: None,
            writes: false,
        };
        assert_eq!(p.badge(), Some(Stage::Dev), "explicit stage beats the name");
        let p = DatabaseProfile {
            name: "production".into(),
            env: "X".into(),
            stage: None,
            pgrun_project: None,
            writes: false,
        };
        assert_eq!(p.badge(), Some(Stage::Prod));
    }

    #[test]
    fn stage_and_ui_round_trip_and_old_files_still_load() {
        let mut cfg = TerminalConfig::default();
        cfg.add_with_stage("prod", "PROD_URL", Some(Stage::Prod))
            .unwrap();
        cfg.add("analytics", "AN_URL").unwrap();
        cfg.ui.bell = true;
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(text.contains("stage = \"prod\""), "{text}");
        assert!(
            !text.contains("stage = \"\""),
            "absent stage must not serialize: {text}"
        );
        let back: TerminalConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, cfg);
        assert!(back.ui.bell && back.ui.sidebar_detail);

        let old = "version = 1\n[[databases]]\nname = \"p\"\nenv = \"P_URL\"\n";
        let cfg: TerminalConfig = toml::from_str(old).unwrap();
        assert_eq!(cfg.databases[0].stage, None);
        assert!(
            cfg.ui.sidebar_detail && !cfg.ui.bell,
            "ui defaults apply to old files"
        );
    }

    #[test]
    fn unknown_stage_is_an_error_naming_the_four() {
        let bad =
            "version = 1\n[[databases]]\nname = \"p\"\nenv = \"P_URL\"\nstage = \"production\"\n";
        let err = toml::from_str::<TerminalConfig>(bad)
            .unwrap_err()
            .to_string();
        for s in ["prod", "staging", "dev", "local"] {
            assert!(err.contains(s), "error should name {s}: {err}");
        }
    }

    #[test]
    fn default_config_text_parses_to_the_defaults() {
        let cfg: TerminalConfig = toml::from_str(DEFAULT_CONFIG_TEXT).unwrap();
        assert_eq!(cfg.settings, Settings::default());
        assert_eq!(cfg.ui, UiSettings::default());
        assert!(cfg.databases.is_empty());
        assert!(
            DEFAULT_CONFIG_TEXT.contains("# stage"),
            "the text is annotated"
        );
    }
}
