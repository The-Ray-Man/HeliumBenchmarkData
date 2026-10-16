//! Parsing of the benchmark suite configuration (e.g. `runner/suite.example.json`).

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use log::{debug, info};
use regex::Regex;
use serde::{Deserialize, Deserializer};

/// Top-level suite configuration file.
///
/// Relative paths in it are resolved against the directory of the file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteConfig {
    /// Toolchain passed to rustup as `+toolchain` for `rustc` and for
    /// rebuilding Helium; `None` uses the default
    /// resolution (`rust-toolchain.toml`, then rustup's default).
    #[serde(default)]
    pub rustc_toolchain: Option<String>,
    /// Command used to start the JVM. A bare name is looked up on `PATH`.
    #[serde(default = "default_java")]
    pub java: PathBuf,
    pub silicon: SiliconConfig,
    /// Helium's `verify` binary.
    pub helium: PathBuf,
    /// Directory holding one folder per suite (`<suites_dir>/<suite>/<family>/...`).
    pub suites_dir: PathBuf,
    /// How often every benchmark is measured.
    #[serde(default = "default_repeat_runs")]
    pub repeat_runs: usize,
    /// Untimed runs before the measured ones, per benchmark and tool.
    #[serde(default = "default_warmup_runs")]
    pub warmup_runs: usize,
    /// Seconds after which a single run is killed; `None` means no limit.
    #[serde(default)]
    pub timeout: Option<f64>,
    pub suites: Vec<Suite>,
}

/// How to run Silicon in a warmed-up JVM.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiliconConfig {
    /// Silicon's fat jar, used as the class path of the warm driver.
    pub jar: PathBuf,
    /// Arguments for the JVM, e.g. `-Xss128m`.
    #[serde(default)]
    pub jvm_args: Vec<String>,
    /// Extra arguments passed to Silicon for every file.
    #[serde(default)]
    pub args: Vec<String>,
    pub warm: WarmConfig,
}

/// How each new Silicon JVM is warmed up before it is measured.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WarmConfig {
    /// Directories searched recursively for warm-up `.vpr` files.
    pub corpus: Vec<PathBuf>,
    /// Seconds spent warming up each new JVM.
    #[serde(default = "default_warmup_s")]
    pub warmup_s: f64,
    /// Silicon's own `--timeout` for each warm-up file, so one slow file
    /// cannot take the whole budget.
    #[serde(default = "default_file_timeout_s")]
    pub file_timeout_s: u64,
}

fn default_activated() -> bool {
    false
}

fn default_java() -> PathBuf {
    "java".into()
}

fn default_warmup_runs() -> usize {
    1
}

fn default_repeat_runs() -> usize {
    5
}

fn default_warmup_s() -> f64 {
    60.0
}

fn default_file_timeout_s() -> u64 {
    10
}

/// A benchmark suite, corresponding to a directory under `suites/`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub name: String,
    /// Whether the suite is run; lets a suite stay in the file while skipped.
    #[serde(default = "default_activated")]
    pub activated: bool,
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
        let mut config: Self =
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
        config.resolve_paths(path.parent().unwrap_or(Path::new("")));
        config
            .check_paths()
            .map_err(|message| SuiteConfigError::Invalid {
                path: path.to_path_buf(),
                message,
            })?;
        let active: Vec<&str> = config.activated_suites().map(|s| s.name.as_str()).collect();
        info!(
            "Loaded {}: {} of {} suites activated ({})",
            path.display(),
            active.len(),
            config.suites.len(),
            active.join(", ")
        );
        debug!("suites in {}", config.suites_dir.display());
        Ok(config)
    }

    /// The per-run timeout, if any.
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout.map(Duration::from_secs_f64)
    }

    pub fn suite(&self, name: &str) -> Option<&Suite> {
        self.suites.iter().find(|s| s.name == name)
    }

    /// The suites to run.
    pub fn activated_suites(&self) -> impl Iterator<Item = &Suite> {
        self.suites.iter().filter(|s| s.activated)
    }

    /// Makes relative paths relative to `base`. A bare `java` command name
    /// is kept so it is looked up on `PATH`.
    fn resolve_paths(&mut self, base: &Path) {
        if self.java.is_relative() && self.java.components().count() > 1 {
            self.java = base.join(&self.java);
        }
        let paths = [&mut self.helium, &mut self.silicon.jar, &mut self.suites_dir]
            .into_iter()
            .chain(&mut self.silicon.warm.corpus);
        for p in paths {
            if p.is_relative() {
                *p = base.join(&*p);
            }
        }
    }

    /// Checks that every configured executable, jar and directory exists.
    fn check_paths(&self) -> Result<(), String> {
        if find_command(&self.java).is_none() {
            return Err(format!("`java`: {} not found", self.java.display()));
        }
        if !self.helium.is_file() {
            return Err(format!("`helium`: {} is not a file", self.helium.display()));
        }
        if !self.silicon.jar.is_file() {
            return Err(format!("`silicon.jar`: {} is not a file", self.silicon.jar.display()));
        }
        for dir in &self.silicon.warm.corpus {
            if !dir.is_dir() {
                return Err(format!("`silicon.warm.corpus`: {} is not a directory", dir.display()));
            }
        }
        for suite in self.activated_suites() {
            for (_, dir) in self.suite_dirs(suite) {
                if !dir.is_dir() {
                    return Err(format!("suite `{}`: {} is not a directory", suite.name, dir.display()));
                }
            }
        }
        Ok(())
    }

    /// The directories holding `suite`'s benchmarks, each with its family:
    /// `<suites_dir>/<suite>/<family>` per family, or `(None, <suites_dir>/<suite>)`
    /// for a flat suite.
    pub fn suite_dirs<'a>(&self, suite: &'a Suite) -> Vec<(Option<&'a Family>, PathBuf)> {
        let root = self.suites_dir.join(&suite.name);
        if suite.is_flat() {
            vec![(None, root)]
        } else {
            suite.families.iter().map(|f| (Some(f), root.join(&f.name))).collect()
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.repeat_runs == 0 {
            return Err("`repeat_runs` must be at least 1".into());
        }
        if let Some(t) = self.timeout
            && !(t.is_finite() && t > 0.0)
        {
            return Err(format!("`timeout` must be a positive number of seconds, got {t}"));
        }
        let warm = &self.silicon.warm;
        if warm.corpus.is_empty() {
            return Err("`silicon.warm.corpus` must name at least one directory".into());
        }
        if !(warm.warmup_s.is_finite() && warm.warmup_s >= 0.0) {
            return Err(format!("`silicon.warm.warmup_s` must not be negative, got {}", warm.warmup_s));
        }
        if warm.file_timeout_s == 0 {
            return Err("`silicon.warm.file_timeout_s` must be at least 1".into());
        }
        if let Some(tc) = &self.rustc_toolchain
            && tc.trim().is_empty()
        {
            return Err("`rustc_toolchain` must not be empty; omit it for the default".into());
        }

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

/// The file `command` runs: a path is taken as is, a bare name is looked up
/// on `PATH` (also with `.exe` on Windows).
fn find_command(command: &Path) -> Option<PathBuf> {
    if command.is_absolute() || command.components().count() > 1 {
        return command.is_file().then(|| command.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(command);
        if candidate.is_file() {
            return Some(candidate);
        }
        let exe = candidate.with_extension(std::env::consts::EXE_EXTENSION);
        (cfg!(windows) && exe.is_file()).then_some(exe)
    })
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

    const SILICON: &str = r#""silicon": {"jar": "silicon.jar", "warm": {"corpus": ["warmup"]}}"#;

    fn parse(json: &str) -> Result<SuiteConfig, String> {
        let config: SuiteConfig = serde_json::from_str(json).map_err(|e| e.to_string())?;
        config.validate()?;
        Ok(config)
    }

    /// A minimal config with the given top-level fields (each ending in `,`) and suites.
    fn config(top: &str, suites: &str) -> Result<SuiteConfig, String> {
        parse(&format!(r#"{{{SILICON}, "helium": "verify", "suites_dir": "suites", {top} "suites": {suites}}}"#))
    }

    fn suites(suites: &str) -> Result<SuiteConfig, String> {
        config("", suites)
    }

    #[test]
    fn parses_example_suite_file() {
        // Parsed without `check_paths`: the example's paths need not exist here.
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("suite.example.json");
        let config = parse(&fs::read_to_string(path).unwrap()).unwrap();

        assert!(config.repeat_runs >= 1);
        assert!(!config.silicon.warm.corpus.is_empty());

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
    #[ignore = "needs runner/suite.local.json and the tools it names"]
    fn loads_local_suite_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("suite.local.json");
        SuiteConfig::new(path).unwrap();
    }

    #[test]
    fn optional_fields_default() {
        let config = suites(r#"[{"name": "s", "families": [{"name": "f"}]}, {"name": "t"}]"#).unwrap();
        assert_eq!(config.rustc_toolchain, None);
        assert_eq!(config.java, Path::new("java"));
        assert_eq!(config.repeat_runs, 5);
        assert_eq!(config.warmup_runs, 1);
        assert_eq!(config.timeout(), None);
        assert!(config.silicon.jvm_args.is_empty() && config.silicon.args.is_empty());
        assert_eq!(config.silicon.warm.warmup_s, 60.0);
        assert_eq!(config.silicon.warm.file_timeout_s, 10);
        assert_eq!(config.suites[0].description, None);
        assert!(config.suites[0].tags.is_empty());
        assert!(config.suites[1].is_flat());
        assert!(config.suites.iter().all(|s| s.activated));
    }

    #[test]
    fn filters_deactivated_suites() {
        let config = suites(r#"[{"name": "a"}, {"name": "b", "activated": false}, {"name": "c", "activated": true}]"#).unwrap();
        let names: Vec<_> = config.activated_suites().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a", "c"]);
        assert!(config.suite("b").is_some());
    }

    #[test]
    fn reads_run_settings() {
        let config = config(
            r#""rustc_toolchain": "nightly-2026-08-19", "java": "/opt/jdk/bin/java", "repeat_runs": 3, "warmup_runs": 0, "timeout": 2.5,"#,
            "[]",
        )
        .unwrap();
        assert_eq!(config.rustc_toolchain.as_deref(), Some("nightly-2026-08-19"));
        assert_eq!(config.java, Path::new("/opt/jdk/bin/java"));
        assert_eq!(config.repeat_runs, 3);
        assert_eq!(config.warmup_runs, 0);
        assert_eq!(config.timeout(), Some(Duration::from_millis(2500)));
    }

    #[test]
    fn resolves_relative_paths_against_config_dir() {
        let mut config = config(r#""java": "java","#, "[]").unwrap();
        config.helium = "../bin/verify".into();
        config.resolve_paths(Path::new("cfg"));
        assert_eq!(config.java, Path::new("java"));
        assert_eq!(config.helium, Path::new("cfg/../bin/verify"));
        assert_eq!(config.silicon.jar, Path::new("cfg/silicon.jar"));
        assert_eq!(config.silicon.warm.corpus, [Path::new("cfg/warmup")]);
        assert_eq!(config.suites_dir, Path::new("cfg/suites"));
    }

    #[test]
    fn rejects_invalid_run_settings() {
        assert!(parse(r#"{"helium": "verify", "suites_dir": "s", "suites": []}"#).is_err());
        assert!(parse(&format!(r#"{{{SILICON}, "helium": "verify", "suites": []}}"#)).is_err());
        assert!(parse(&format!(r#"{{{SILICON}, "suites": []}}"#)).is_err());
        assert!(config(r#""repeat_runs": 0,"#, "[]").is_err());
        assert!(config(r#""timeout": 0,"#, "[]").is_err());
        assert!(config(r#""timeout": -1,"#, "[]").is_err());
        assert!(config(r#""rustc_toolchain": "","#, "[]").is_err());
        let silicon = |warm: &str| {
            parse(&format!(r#"{{"silicon": {{"jar": "s.jar", "warm": {warm}}}, "helium": "v", "suites_dir": "s", "suites": []}}"#))
        };
        assert!(silicon(r#"{"corpus": []}"#).is_err());
        assert!(silicon(r#"{"corpus": ["c"], "warmup_s": -1}"#).is_err());
        assert!(silicon(r#"{"corpus": ["c"], "file_timeout_s": 0}"#).is_err());
        assert!(silicon(r#"{"corpus": ["c"]}"#).is_ok());
    }

    #[test]
    fn rejects_invalid_suites() {
        assert!(suites(r#"[{"description": "no name"}]"#).is_err());
        assert!(suites(r#"[{"name": "s"}, {"name": "s"}]"#).is_err());
        assert!(suites(r#"[{"name": "s", "families": [{"name": "f"}, {"name": "f"}]}]"#).is_err());
        assert!(suites(r#"[{"name": "s", "families": [{"name": "f", "knobs": ["k"]}]}]"#).is_err());
        assert!(suites(r#"[{"name": "s", "families": [{"name": "f", "pattern": "x(?P<a>\d+)", "knobs": ["b"]}]}]"#).is_err());
        assert!(suites(r#"[{"name": "s", "typo": 1}]"#).is_err());
        assert!(suites(r#"[{"name": "s", "families": [{"name": "f", "pattern": "x(?P<a>[0-9]+"}]}]"#).is_err());
    }

    #[test]
    fn checks_that_paths_exist() {
        let dir = std::env::temp_dir().join(format!("suite_config_test_{}", std::process::id()));
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join("warmup")).unwrap();
        fs::create_dir_all(dir.join("suites/flat")).unwrap();
        fs::write(dir.join("bin/verify"), "").unwrap();
        fs::write(dir.join("silicon.jar"), "").unwrap();
        let java = std::env::current_exe().unwrap();
        let write = |helium: &str, corpus: &str| {
            let json = serde_json::json!({
                "java": java,
                "silicon": {"jar": "silicon.jar", "warm": {"corpus": [corpus]}},
                "helium": helium,
                "suites_dir": "suites",
                "suites": [{"name": "flat"}, {"name": "off", "activated": false}],
            });
            fs::write(dir.join("suite.json"), json.to_string()).unwrap();
            SuiteConfig::new(dir.join("suite.json"))
        };

        let config = write("bin/verify", "warmup").unwrap();
        assert_eq!(config.helium, dir.join("bin/verify"));
        assert!(matches!(write("bin/missing", "warmup"), Err(SuiteConfigError::Invalid { .. })));
        assert!(matches!(write("bin/verify", "missing"), Err(SuiteConfigError::Invalid { .. })));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finds_commands() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(find_command(&exe), Some(exe.clone()));
        assert_eq!(find_command(Path::new("no/such/binary")), None);
        assert_eq!(find_command(Path::new("no-such-binary-on-path")), None);
    }

    #[test]
    fn missing_file_is_io_error() {
        assert!(matches!(
            SuiteConfig::new("does/not/exist.json"),
            Err(SuiteConfigError::Io { .. })
        ));
    }
}
