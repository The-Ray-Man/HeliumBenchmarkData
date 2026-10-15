//! Parsing of the benchmark suite configuration (e.g. `suites/suite.json`).

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Deserializer};

/// Top-level suite configuration file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteConfig {
    pub suites: Vec<Suite>,
}

/// A benchmark suite, corresponding to a directory under `suites/`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Empty for flat suites that have no family level.
    #[serde(default)]
    pub families: Vec<Family>,
}

/// A group of related benchmark files within a suite.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Family {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Regex with named groups extracting knob values from a file name.
    /// Compiled while loading, so an invalid regex fails the load.
    #[serde(default, deserialize_with = "deserialize_regex")]
    pub pattern: Option<Regex>,
    /// Names of the knobs captured by `pattern`.
    #[serde(default)]
    pub knobs: Vec<String>,
}

#[derive(Debug)]
pub enum SuiteConfigError {
    Io { path: PathBuf, source: std::io::Error },
    Parse { path: PathBuf, source: serde_json::Error },
    Invalid { path: PathBuf, message: String },
}

impl fmt::Display for SuiteConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to read suite config {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(f, "failed to parse suite config {}: {source}", path.display())
            }
            Self::Invalid { path, message } => {
                write!(f, "invalid suite config {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for SuiteConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Invalid { .. } => None,
        }
    }
}

impl SuiteConfig {
    /// Reads, parses and validates the suite configuration at `path`.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, SuiteConfigError> {
        let path = path.as_ref();
        let content = fs::read_to_string(path).map_err(|source| SuiteConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Self =
            serde_json::from_str(&content).map_err(|source| SuiteConfigError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        config
            .validate()
            .map_err(|message| SuiteConfigError::Invalid {
                path: path.to_path_buf(),
                message,
            })?;
        Ok(config)
    }

    pub fn suite(&self, name: &str) -> Option<&Suite> {
        self.suites.iter().find(|s| s.name == name)
    }

    fn validate(&self) -> Result<(), String> {
        let mut suite_names = HashSet::new();
        for suite in &self.suites {
            if suite.name.is_empty() {
                return Err("suite with empty name".into());
            }
            if !suite_names.insert(&suite.name) {
                return Err(format!("duplicate suite `{}`", suite.name));
            }
            let mut family_names = HashSet::new();
            for family in &suite.families {
                if family.name.is_empty() {
                    return Err(format!("family with empty name in suite `{}`", suite.name));
                }
                if !family_names.insert(&family.name) {
                    return Err(format!(
                        "duplicate family `{}` in suite `{}`",
                        family.name, suite.name
                    ));
                }
                family.validate_knobs().map_err(|e| {
                    format!("family `{}` in suite `{}`: {e}", family.name, suite.name)
                })?;
            }
        }
        Ok(())
    }
}

impl Suite {
    pub fn family(&self, name: &str) -> Option<&Family> {
        self.families.iter().find(|f| f.name == name)
    }

    /// Whether the suite has no family level (benchmark files sit directly in the suite).
    pub fn is_flat(&self) -> bool {
        self.families.is_empty()
    }
}

impl Family {
    /// Every knob must be captured by a named group of `pattern`.
    fn validate_knobs(&self) -> Result<(), String> {
        if self.knobs.is_empty() {
            return Ok(());
        }
        let Some(pattern) = &self.pattern else {
            return Err("`knobs` given without a `pattern`".into());
        };
        for knob in &self.knobs {
            if !pattern.capture_names().flatten().any(|name| name == knob) {
                return Err(format!("knob `{knob}` is not a named group in pattern `{pattern}`"));
            }
        }
        Ok(())
    }
}

fn deserialize_regex<'de, D>(deserializer: D) -> Result<Option<Regex>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .map(|pattern| Regex::new(&pattern).map_err(serde::de::Error::custom))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<SuiteConfig, String> {
        let config: SuiteConfig = serde_json::from_str(json).map_err(|e| e.to_string())?;
        config.validate()?;
        Ok(config)
    }

    #[test]
    fn loads_example_suite_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../suites/suite.json");
        let config = SuiteConfig::new(&path).unwrap();

        let flat = config.suite("cratesTop100").unwrap();
        assert!(flat.is_flat());
        assert_eq!(flat.tags, ["prusti", "crates"]);

        let depth = config.suite("scaling").unwrap().family("depth").unwrap();
        assert_eq!(depth.knobs, ["depth", "stmts"]);
        let caps = depth.pattern.as_ref().unwrap().captures("depth_d3_m12").unwrap();
        assert_eq!((&caps["depth"], &caps["stmts"]), ("3", "12"));

        let dag = config.suite("marco_benchmark").unwrap().family("DaG").unwrap();
        assert!(dag.pattern.is_none());
        assert!(dag.knobs.is_empty());
    }

    #[test]
    fn optional_fields_default() {
        let config = parse(r#"{"suites": [{"name": "s", "families": [{"name": "f"}]}, {"name": "t"}]}"#).unwrap();
        assert_eq!(config.suites[0].description, None);
        assert!(config.suites[0].tags.is_empty());
        assert!(config.suites[1].is_flat());
    }

    #[test]
    fn rejects_invalid_configs() {
        assert!(parse(r#"{"suites": [{"description": "no name"}]}"#).is_err());
        assert!(parse(r#"{"suites": [{"name": "s"}, {"name": "s"}]}"#).is_err());
        assert!(parse(r#"{"suites": [{"name": "s", "families": [{"name": "f"}, {"name": "f"}]}]}"#).is_err());
        assert!(parse(r#"{"suites": [{"name": "s", "families": [{"name": "f", "knobs": ["k"]}]}]}"#).is_err());
        assert!(parse(r#"{"suites": [{"name": "s", "families": [{"name": "f", "pattern": "x(?P<a>\\d+)", "knobs": ["b"]}]}]}"#).is_err());
        assert!(parse(r#"{"suites": [{"name": "s", "typo": 1}]}"#).is_err());
        assert!(parse(r#"{"suites": [{"name": "s", "families": [{"name": "f", "pattern": "x(?P<a>[0-9]+"}]}]}"#).is_err());
    }

    #[test]
    fn missing_file_is_io_error() {
        assert!(matches!(
            SuiteConfig::new("does/not/exist.json"),
            Err(SuiteConfigError::Io { .. })
        ));
    }
}
