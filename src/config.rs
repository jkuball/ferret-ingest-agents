//! Settings from a TOML file. The API key comes from `key_file` or `FERRET_KEY`.
//! Every `[harnesses.<name>]` table turns that harness on.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::Deserialize;

use crate::harnesses::Harness;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    url: String,
    key_file: Option<String>,
    state_file: Option<String>,
    /// Names this machine in sources; defaults to the short hostname.
    host: Option<String>,
    /// The person the sessions are with, an author of every turn; defaults to $USER.
    user: Option<String>,
    /// A minijinja template replacing the built-in one.
    turn_template: Option<String>,
    #[serde(default)]
    harnesses: BTreeMap<String, HarnessFile>,
    #[serde(default = "everything")]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default = "four")]
    concurrency: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HarnessFile {
    sessions_dir: Option<String>,
    source: Option<String>,
}

fn everything() -> Vec<String> {
    vec!["**".to_owned()]
}
fn four() -> usize {
    4
}

pub struct Config {
    pub url: String,
    pub key: String,
    pub host: String,
    pub user: String,
    pub turn_template: Option<PathBuf>,
    pub harnesses: Vec<HarnessConfig>,
    pub state_file: PathBuf,
    pub concurrency: usize,
    include: GlobSet,
    exclude: GlobSet,
    include_patterns: Vec<String>,
    exclude_patterns: Vec<String>,
}

pub struct HarnessConfig {
    pub harness: Harness,
    pub sessions_dir: PathBuf,
    /// The ferret source its documents go to.
    pub source: String,
}

impl Config {
    pub fn default_path() -> PathBuf {
        xdg("XDG_CONFIG_HOME", ".config").join("ferret-ingest-agents/config.toml")
    }

    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the config {}", path.display()))?;
        let file: File =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

        let key = match std::env::var("FERRET_KEY") {
            Ok(key) if !key.trim().is_empty() => key.trim().to_owned(),
            _ => {
                let Some(key_file) = &file.key_file else {
                    bail!("set key_file in {} or FERRET_KEY", path.display());
                };
                let key_file = expand(key_file);
                std::fs::read_to_string(&key_file)
                    .with_context(|| format!("reading key_file {}", key_file.display()))?
                    .trim()
                    .to_owned()
            }
        };
        if key.is_empty() {
            bail!("the ferret API key is empty");
        }
        if file.concurrency == 0 {
            bail!("concurrency must be at least 1");
        }

        let host = file.host.unwrap_or_else(default_host);
        if host.is_empty() || host.contains('/') {
            bail!("host must be one source segment, not {host:?}");
        }
        let user = file
            .user
            .or_else(|| std::env::var("USER").ok())
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| "user".to_owned());

        if file.harnesses.is_empty() {
            bail!(
                "{}: turn on at least one harness with a [harnesses.<name>] table",
                path.display()
            );
        }
        let mut harnesses = Vec::new();
        for (name, settings) in file.harnesses {
            let Some(harness) = Harness::from_name(&name) else {
                let known: Vec<_> = Harness::ALL.iter().map(|h| h.name()).collect();
                bail!("unknown harness {name:?}, known are {}", known.join(", "));
            };
            harnesses.push(HarnessConfig {
                harness,
                sessions_dir: expand(
                    settings
                        .sessions_dir
                        .as_deref()
                        .unwrap_or(harness.default_dir()),
                ),
                source: settings
                    .source
                    .unwrap_or_else(|| default_source(&host, harness)),
            });
        }

        Ok(Config {
            url: file.url.trim_end_matches('/').to_owned(),
            key,
            host,
            user,
            turn_template: file.turn_template.as_deref().map(expand),
            harnesses,
            state_file: file.state_file.as_deref().map(expand).unwrap_or_else(|| {
                xdg("XDG_STATE_HOME", ".local/state").join("ferret-ingest-agents/state.json")
            }),
            concurrency: file.concurrency,
            include: patterns(&file.include).context("include")?,
            exclude: patterns(&file.exclude).context("exclude")?,
            include_patterns: file.include,
            exclude_patterns: file.exclude,
        })
    }

    /// The patterns, to tell when the set of indexed sessions may differ.
    pub fn include_exclude(&self) -> (&[String], &[String]) {
        (&self.include_patterns, &self.exclude_patterns)
    }

    /// Whether sessions started in `cwd` are indexed.
    pub fn wants(&self, cwd: &str) -> bool {
        self.include.is_match(cwd) && !self.exclude.is_match(cwd)
    }
}

/// Globs on the session's working directory. `*` stays within one path
/// segment, `**` crosses them, and `dir/**` also matches `dir` itself.
pub fn patterns(globs: &[String]) -> Result<GlobSet> {
    let mut set = GlobSetBuilder::new();
    for raw in globs {
        let pattern = expand(raw).to_string_lossy().into_owned();
        let mut variants = vec![pattern.clone()];
        if let Some(base) = pattern.strip_suffix("/**") {
            variants.push(base.to_owned());
        }
        for variant in variants {
            let glob = GlobBuilder::new(&variant)
                .literal_separator(true)
                .build()
                .with_context(|| format!("pattern {raw:?}"))?;
            set.add(glob);
        }
    }
    Ok(set.build()?)
}

pub fn default_host() -> String {
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    host.split('.').next().unwrap_or("unknown").to_lowercase()
}

/// `<host>/agents/<harness>`: each machine runs its own ingester, so one key
/// scoped to `<host>` covers all of that machine's documents.
pub fn default_source(host: &str, harness: Harness) -> String {
    format!("{host}/agents/{}", harness.name())
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home().join(fallback),
    }
}

pub fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if path == "~" => home(),
        None => PathBuf::from(path),
    }
}

/// A path with the home directory shown as `~`, as in documents and the state.
pub fn display(path: &Path) -> String {
    let home = home();
    match path.strip_prefix(&home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(globs: &[&str], cwd: &str) -> bool {
        let globs: Vec<String> = globs.iter().map(|g| g.to_string()).collect();
        patterns(&globs).unwrap().is_match(cwd)
    }

    #[test]
    fn a_double_star_suffix_covers_the_directory_and_everything_below() {
        assert!(matches(&["/work/**"], "/work"));
        assert!(matches(&["/work/**"], "/work/a/b"));
        assert!(!matches(&["/work/**"], "/workshop"));
    }

    #[test]
    fn a_single_star_stays_within_one_segment() {
        assert!(matches(&["/src/*"], "/src/a"));
        assert!(!matches(&["/src/*"], "/src/a/b"));
    }

    #[test]
    fn everything_matches_double_star() {
        assert!(matches(&["**"], "/anything/at/all"));
    }

    #[test]
    fn the_host_comes_first_in_a_source() {
        assert_eq!(
            default_source("laptop", Harness::ClaudeCode),
            "laptop/agents/claude-code"
        );
    }
}
