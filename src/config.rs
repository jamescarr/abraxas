//! Configuration: file locations, `config.toml`, and CLI overrides.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::cli::GlobalOpts;

/// Where files live. `ABRAXAS_HOME`, when set, holds everything; otherwise the platform's
/// config and data directories for `abraxas`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Self {
        if let Some(home) = std::env::var_os("ABRAXAS_HOME").filter(|v| !v.is_empty()) {
            return Self::in_dir(PathBuf::from(home));
        }
        match directories::ProjectDirs::from("", "", "abraxas") {
            Some(dirs) => Self {
                config_dir: dirs.config_dir().to_path_buf(),
                data_dir: dirs.data_dir().to_path_buf(),
            },
            None => Self::in_dir(PathBuf::from(".abraxas")),
        }
    }

    /// Everything under one directory (used for `ABRAXAS_HOME` and tests).
    pub fn in_dir(dir: PathBuf) -> Self {
        Self {
            config_dir: dir.clone(),
            data_dir: dir,
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn saves_dir(&self) -> PathBuf {
        self.data_dir.join("saves")
    }

    pub fn callers_file(&self) -> PathBuf {
        self.data_dir.join("callers.json")
    }

    pub fn log_file(&self) -> PathBuf {
        self.data_dir.join("abraxas.log")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormatKind {
    JsonSchema,
    JsonObject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiConfig {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    /// Empty = use `model`.
    pub chat_model: String,
    /// Name of the env var holding the API key; empty = no auth header.
    pub api_key_env: String,
    pub temperature: f32,
    /// Empty = omit the parameter.
    pub reasoning_effort: String,
    pub response_format: ResponseFormatKind,
    pub timeout_secs: u64,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: "http://localhost:11434/v1".to_string(),
            model: "qwen3.8".to_string(),
            chat_model: String::new(),
            api_key_env: String::new(),
            temperature: 0.9,
            reasoning_effort: "none".to_string(),
            response_format: ResponseFormatKind::JsonSchema,
            timeout_secs: 300,
        }
    }
}

impl AiConfig {
    /// The model for chat calls.
    pub fn chat_model(&self) -> &str {
        if self.chat_model.is_empty() {
            &self.model
        } else {
            &self.chat_model
        }
    }

    pub fn reasoning_effort(&self) -> Option<&str> {
        (!self.reasoning_effort.is_empty()).then_some(self.reasoning_effort.as_str())
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TruecolorMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisplayConfig {
    /// 0 = instant.
    pub baud: u32,
    pub calm: bool,
    pub crt: bool,
    pub truecolor: TruecolorMode,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            baud: 9600,
            calm: false,
            crt: true,
            truecolor: TruecolorMode::Auto,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub ai: AiConfig,
    pub display: DisplayConfig,
}

impl Config {
    /// CLI flags override the file, which overrides defaults.
    pub fn apply_cli(&mut self, opts: &GlobalOpts) {
        if opts.offline {
            self.ai.enabled = false;
        }
        if let Some(model) = &opts.model {
            self.ai.model = model.clone();
        }
        if let Some(url) = &opts.base_url {
            self.ai.base_url = url.clone();
        }
        if opts.calm {
            self.display.calm = true;
        }
        if let Some(baud) = opts.baud {
            self.display.baud = baud;
        }
    }
}

/// Load `config.toml`. A missing file means defaults; a parse error is returned so the caller
/// can print it and exit before the TUI starts.
pub fn load(paths: &Paths) -> anyhow::Result<Config> {
    load_file(&paths.config_file())
}

fn load_file(path: &Path) -> anyhow::Result<Config> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            toml::from_str(&text).with_context(|| format!("invalid config {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("cannot read config {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = load(&Paths::in_dir(dir.path().to_path_buf())).unwrap();
        assert_eq!(cfg, Config::default());
        assert_eq!(cfg.ai.model, "qwen3.8");
        assert_eq!(cfg.display.baud, 9600);
    }

    #[test]
    fn partial_file_overrides_only_what_it_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[ai]\nmodel = \"llama3.2:3b\"\n[display]\ncrt = false\n",
        )
        .unwrap();
        let cfg = load(&Paths::in_dir(dir.path().to_path_buf())).unwrap();
        assert_eq!(cfg.ai.model, "llama3.2:3b");
        assert!(cfg.ai.enabled);
        assert!(!cfg.display.crt);
        assert_eq!(cfg.display.baud, 9600);
    }

    #[test]
    fn parse_error_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[ai]\nbogus = 1\n").unwrap();
        let err = load(&Paths::in_dir(dir.path().to_path_buf())).unwrap_err();
        assert!(format!("{err:#}").contains("config.toml"));
    }

    #[test]
    fn cli_flags_override_config() {
        let mut cfg = Config::default();
        let opts = GlobalOpts {
            offline: true,
            model: Some("m".into()),
            base_url: Some("http://x/v1".into()),
            calm: true,
            baud: Some(0),
            ..GlobalOpts::default()
        };
        cfg.apply_cli(&opts);
        assert!(!cfg.ai.enabled);
        assert_eq!(cfg.ai.model, "m");
        assert_eq!(cfg.ai.base_url, "http://x/v1");
        assert!(cfg.display.calm);
        assert_eq!(cfg.display.baud, 0);
    }

    #[test]
    fn empty_chat_model_falls_back_to_model() {
        let mut ai = AiConfig::default();
        assert_eq!(ai.chat_model(), "qwen3.8");
        ai.chat_model = "tiny".into();
        assert_eq!(ai.chat_model(), "tiny");
        assert_eq!(AiConfig::default().reasoning_effort(), Some("none"));
    }
}
