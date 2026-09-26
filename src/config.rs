//! Cockpit configuration: which agents can be spawned and which tool panes open
//! next to the dashboard.
//!
//! The file is plain data and never written by harnesh. Nix modules, other
//! package managers or a user generate it; without one, harnesh falls back to
//! built-in presets for agent commands found on `PATH`.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const PROMPT_PLACEHOLDER: &str = "{prompt}";
pub const DEFAULT_SESSION: &str = "harnesh";
pub const DEFAULT_MAX_AGENTS: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Adapter {
    /// Human-readable name shown in the dashboard.
    #[serde(default)]
    pub label: Option<String>,
    /// Command that starts an interactive agent session.
    pub argv: Vec<String>,
    /// Command that starts a session with an initial prompt. Every argument
    /// containing `{prompt}` receives the prompt text.
    #[serde(default)]
    pub prompt_argv: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    #[serde(default = "default_session")]
    pub session: String,
    #[serde(default = "default_max_agents")]
    pub max_agents: usize,
    /// Add built-in presets for agent commands found on `PATH`. Configured
    /// adapters with the same name take precedence.
    #[serde(default)]
    pub detect_presets: bool,
    #[serde(default)]
    pub adapters: BTreeMap<String, Adapter>,
    /// Panes opened next to the dashboard in the control window.
    #[serde(default)]
    pub tools: Vec<Tool>,
}

fn default_session() -> String {
    DEFAULT_SESSION.to_owned()
}

fn default_max_agents() -> usize {
    DEFAULT_MAX_AGENTS
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            session: default_session(),
            max_agents: default_max_agents(),
            detect_presets: true,
            adapters: BTreeMap::new(),
            tools: Vec::new(),
        }
    }
}

/// Where a loaded configuration came from, for `harnesh config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    BuiltIn,
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// Generic presets that only name a command on `PATH`. They carry no host
/// paths, flags or credentials.
pub fn presets() -> BTreeMap<String, Adapter> {
    let preset = |label: &str, argv: &[&str], prompt: Option<&[&str]>| Adapter {
        label: Some(label.to_owned()),
        argv: strings(argv),
        prompt_argv: prompt.map(strings),
    };
    BTreeMap::from([
        (
            "claude".to_owned(),
            preset("Claude Code", &["claude"], Some(&["claude", "{prompt}"])),
        ),
        (
            "codex".to_owned(),
            preset("Codex", &["codex"], Some(&["codex", "{prompt}"])),
        ),
        (
            "opencode".to_owned(),
            preset(
                "OpenCode",
                &["opencode"],
                Some(&["opencode", "--prompt", "{prompt}"]),
            ),
        ),
        (
            "gemini".to_owned(),
            preset(
                "Gemini CLI",
                &["gemini"],
                Some(&["gemini", "-i", "{prompt}"]),
            ),
        ),
        (
            "pi".to_owned(),
            preset("Pi", &["pi"], Some(&["pi", "{prompt}"])),
        ),
        ("aider".to_owned(), preset("Aider", &["aider"], None)),
    ])
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported config version {} (expected 1)", self.version);
        }
        if !valid_name(&self.session) {
            bail!("invalid session name {:?}", self.session);
        }
        if self.max_agents == 0 {
            bail!("maxAgents must be at least 1");
        }
        for (name, adapter) in &self.adapters {
            if !valid_name(name) {
                bail!("invalid adapter name {name:?}");
            }
            if adapter.argv.is_empty() {
                bail!("adapter {name} has an empty argv");
            }
            if let Some(prompt) = &adapter.prompt_argv {
                if !prompt.iter().any(|arg| arg.contains(PROMPT_PLACEHOLDER)) {
                    bail!("adapter {name}: promptArgv must contain {PROMPT_PLACEHOLDER}");
                }
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for tool in &self.tools {
            if !valid_name(&tool.name) || tool.name == "dash" {
                bail!("invalid tool name {:?}", tool.name);
            }
            if !seen.insert(&tool.name) {
                bail!("duplicate tool name {:?}", tool.name);
            }
            if tool.argv.is_empty() {
                bail!("tool {} has an empty argv", tool.name);
            }
        }
        Ok(())
    }

    /// Adds presets whose command exists on `path_var`, keeping configured
    /// adapters of the same name.
    pub fn with_detected_presets(mut self, path_var: Option<&std::ffi::OsStr>) -> Self {
        if !self.detect_presets {
            return self;
        }
        for (name, adapter) in presets() {
            if self.adapters.contains_key(&name) {
                continue;
            }
            if find_on_path(&adapter.argv[0], path_var).is_some() {
                self.adapters.insert(name, adapter);
            }
        }
        self
    }
}

pub fn find_on_path(command: &str, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    if command.contains('/') {
        let path = PathBuf::from(command);
        return is_executable(&path).then_some(path);
    }
    env::split_paths(path_var?)
        .map(|dir| dir.join(command))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Candidate config files in lookup order.
pub fn search_paths(explicit: Option<&Path>) -> Vec<PathBuf> {
    if let Some(path) = explicit {
        return vec![path.to_owned()];
    }
    let mut paths = Vec::new();
    if let Some(dir) = env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        paths.push(PathBuf::from(dir).join("harnesh/config.json"));
    } else if let Some(home) = env::var_os("HOME").filter(|home| !home.is_empty()) {
        paths.push(PathBuf::from(home).join(".config/harnesh/config.json"));
    }
    paths.push(PathBuf::from("/etc/harnesh/config.json"));
    paths
}

pub fn parse(text: &str) -> Result<Config> {
    let config: Config = serde_json::from_str(text).context("invalid harnesh config")?;
    config.validate()?;
    Ok(config)
}

/// Loads the first existing config file. An explicitly named file must exist.
pub fn load(explicit: Option<&Path>) -> Result<(Config, Source)> {
    let path_var = env::var_os("PATH");
    for path in search_paths(explicit) {
        match fs::read_to_string(&path) {
            Ok(text) => {
                let config =
                    parse(&text).with_context(|| format!("while reading {}", path.display()))?;
                return Ok((
                    config.with_detected_presets(path_var.as_deref()),
                    Source::File(path),
                ));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && explicit.is_none() => {}
            Err(err) => return Err(err).with_context(|| format!("cannot read {}", path.display())),
        }
    }
    Ok((
        Config::default().with_detected_presets(path_var.as_deref()),
        Source::BuiltIn,
    ))
}

impl Adapter {
    /// The command to run, with the prompt substituted when one is given.
    pub fn command(&self, name: &str, prompt: Option<&str>) -> Result<Vec<String>> {
        match prompt {
            None => Ok(self.argv.clone()),
            Some(prompt) => {
                let Some(template) = &self.prompt_argv else {
                    bail!(
                        "adapter {name} has no promptArgv; start it without --prompt and use `harnesh send`"
                    );
                };
                Ok(template
                    .iter()
                    .map(|arg| arg.replace(PROMPT_PLACEHOLDER, prompt))
                    .collect())
            }
        }
    }

    pub fn display_name<'a>(&'a self, name: &'a str) -> &'a str {
        self.label.as_deref().unwrap_or(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config_with_defaults() {
        let config = parse(r#"{"version":1}"#).unwrap();
        assert_eq!(config.session, "harnesh");
        assert_eq!(config.max_agents, 6);
        assert!(!config.detect_presets);
        assert!(config.adapters.is_empty());
    }

    #[test]
    fn rejects_unknown_fields_and_versions() {
        assert!(parse(r#"{"version":1,"adapter":{}}"#).is_err());
        assert!(parse(r#"{"version":2}"#).is_err());
    }

    #[test]
    fn rejects_prompt_template_without_placeholder() {
        let text = r#"{"version":1,"adapters":{"x":{"argv":["x"],"promptArgv":["x","-p"]}}}"#;
        let err = parse(text).unwrap_err();
        assert!(format!("{err:#}").contains("{prompt}"));
    }

    #[test]
    fn rejects_invalid_and_duplicate_names() {
        assert!(parse(r#"{"version":1,"adapters":{"a b":{"argv":["x"]}}}"#).is_err());
        assert!(parse(r#"{"version":1,"adapters":{"a":{"argv":[]}}}"#).is_err());
        let dup = r#"{"version":1,"tools":[{"name":"t","argv":["x"]},{"name":"t","argv":["y"]}]}"#;
        assert!(parse(dup).is_err());
        assert!(parse(r#"{"version":1,"tools":[{"name":"dash","argv":["x"]}]}"#).is_err());
        assert!(parse(r#"{"version":1,"session":"a:b"}"#).is_err());
    }

    #[test]
    fn substitutes_prompt_in_every_matching_argument() {
        let adapter = Adapter {
            label: None,
            argv: strings(&["agent"]),
            prompt_argv: Some(strings(&["agent", "--prompt", "{prompt}", "x={prompt}"])),
        };
        assert_eq!(adapter.command("a", None).unwrap(), strings(&["agent"]));
        assert_eq!(
            adapter.command("a", Some("fix it")).unwrap(),
            strings(&["agent", "--prompt", "fix it", "x=fix it"])
        );
        let plain = Adapter {
            prompt_argv: None,
            ..adapter
        };
        assert!(plain.command("a", Some("x")).is_err());
    }

    #[test]
    fn presets_are_valid_and_path_only() {
        let config = Config {
            adapters: presets(),
            ..Config::default()
        };
        config.validate().unwrap();
        for adapter in config.adapters.values() {
            assert!(!adapter.argv[0].contains('/'));
        }
    }

    #[test]
    fn detects_only_presets_on_path_and_keeps_configured_adapters() {
        let dir = std::env::temp_dir().join(format!("harnesh-path-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for name in ["codex", "claude"] {
            let file = dir.join(name);
            fs::write(&file, "#!/bin/sh\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut config = Config::default();
        config.adapters.insert(
            "claude".to_owned(),
            Adapter {
                label: Some("mine".to_owned()),
                argv: strings(&["/custom/claude"]),
                prompt_argv: None,
            },
        );
        let config = config.with_detected_presets(Some(dir.as_os_str()));
        fs::remove_dir_all(&dir).unwrap();
        let names: Vec<_> = config.adapters.keys().cloned().collect();
        assert_eq!(names, strings(&["claude", "codex"]));
        assert_eq!(config.adapters["claude"].label.as_deref(), Some("mine"));

        let disabled = Config {
            detect_presets: false,
            ..Config::default()
        }
        .with_detected_presets(Some(std::ffi::OsStr::new("/nonexistent")));
        assert!(disabled.adapters.is_empty());
    }

    #[test]
    fn explicit_missing_config_is_an_error() {
        assert!(load(Some(Path::new("/nonexistent/harnesh.json"))).is_err());
    }
}
