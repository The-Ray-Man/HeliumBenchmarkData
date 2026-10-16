//! Finds the benchmarks of a [`SuiteConfig`]: every `.rs` / `.vpr` file, with
//! a Rust file and a Viper file paired up when they share a directory and a
//! file stem (`args_mut_n1.rs` + `args_mut_n1.vpr`).
//!
//! [`Explorer`] is lazy: it reads one directory per step, so a run can start
//! on the first benchmarks before the large suites are fully listed.

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use log::{debug, trace};

use crate::suite_config::SuiteConfig;

/// One benchmark: a Rust program, its Viper encoding, or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Benchmark {
    pub suite: String,
    /// `None` for flat suites.
    pub family: Option<String>,
    /// Path below the family (or flat suite) directory without extension,
    /// `/`-separated, e.g. `gobra/dafny_book/ex1`. Unique within a family.
    pub test: String,
    pub rust: Option<PathBuf>,
    pub viper: Option<PathBuf>,
}

/// A directory still to be read, and where its benchmarks belong.
struct Pending {
    suite: String,
    family: Option<String>,
    /// The family (or flat suite) directory that `test` names are relative to.
    root: PathBuf,
    dir: PathBuf,
}

/// Iterator over the benchmarks of every activated suite, in suite and family
/// order as configured, then by path. Unreadable directories are yielded as
/// errors and skipped.
pub struct Explorer {
    /// Directories to read; the front is read next.
    pending: VecDeque<Pending>,
    /// Benchmarks of the directory read last, not yet yielded.
    ready: VecDeque<Benchmark>,
}

impl Explorer {
    pub fn new(config: &SuiteConfig) -> Self {
        let mut pending = VecDeque::new();
        for suite in config.activated_suites() {
            for (family, root) in config.suite_dirs(suite) {
                pending.push_back(Pending {
                    suite: suite.name.clone(),
                    family: family.map(|f| f.name.clone()),
                    dir: root.clone(),
                    root,
                });
            }
        }
        Explorer {
            pending,
            ready: VecDeque::new(),
        }
    }

    /// Reads `next`: its benchmarks go to `ready`, its subdirectories to the
    /// front of `pending` so each family is finished depth-first before the next.
    fn read(&mut self, next: Pending) -> io::Result<()> {
        trace!("reading {}", next.dir.display());
        let mut subdirs = Vec::new();
        let mut pairs: BTreeMap<String, (Option<PathBuf>, Option<PathBuf>)> = BTreeMap::new();
        for entry in fs::read_dir(&next.dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                subdirs.push(path);
                continue;
            }
            let (Some(stem), Some(ext)) = (path.file_stem(), path.extension()) else {
                continue;
            };
            let stem = stem.to_string_lossy().into_owned();
            match ext.to_str() {
                Some("rs") => pairs.entry(stem).or_default().0 = Some(path),
                Some("vpr") => pairs.entry(stem).or_default().1 = Some(path),
                _ => {}
            }
        }

        if !pairs.is_empty() {
            debug!("{}: {} benchmarks", next.dir.display(), pairs.len());
        }
        let prefix = test_prefix(&next.root, &next.dir);
        self.ready.extend(pairs.into_iter().map(|(stem, (rust, viper))| Benchmark {
            suite: next.suite.clone(),
            family: next.family.clone(),
            test: format!("{prefix}{stem}"),
            rust,
            viper,
        }));

        subdirs.sort();
        for dir in subdirs.into_iter().rev() {
            self.pending.push_front(Pending {
                suite: next.suite.clone(),
                family: next.family.clone(),
                root: next.root.clone(),
                dir,
            });
        }
        Ok(())
    }
}

impl Iterator for Explorer {
    type Item = io::Result<Benchmark>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(benchmark) = self.ready.pop_front() {
                return Some(Ok(benchmark));
            }
            let next = self.pending.pop_front()?;
            let dir = next.dir.clone();
            if let Err(e) = self.read(next) {
                let message = format!("reading {}: {e}", dir.display());
                return Some(Err(io::Error::new(e.kind(), message)));
            }
        }
    }
}

/// `dir` relative to `root` as a `/`-terminated prefix (`""` for `root` itself).
fn test_prefix(root: &Path, dir: &Path) -> String {
    let rel = dir.strip_prefix(root).unwrap_or(Path::new(""));
    rel.components()
        .map(|c| format!("{}/", c.as_os_str().to_string_lossy()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(suites_dir: &Path, suites: serde_json::Value) -> SuiteConfig {
        let mut config: SuiteConfig = serde_json::from_value(serde_json::json!({
            "silicon": {"jar": "s.jar", "warm": {"corpus": ["c"]}},
            "helium": "verify",
            "suites_dir": "",
            "suites": suites,
        }))
        .unwrap();
        config.suites_dir = suites_dir.to_path_buf();
        config
    }

    fn touch(root: &Path, files: &[&str]) {
        for f in files {
            let path = root.join(f);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
    }

    #[test]
    fn pairs_files_and_skips_deactivated_suites() {
        let root = std::env::temp_dir().join(format!("explorer_test_{}", std::process::id()));
        touch(
            &root,
            &[
                "flat/a.vpr",
                "flat/notes.json",
                "fam/f1/x.rs",
                "fam/f1/x.vpr",
                "fam/f1/only_rust.rs",
                "fam/f1/sub/deep.vpr",
                "fam/f1/sub/more/deeper.vpr",
                "fam/f1/z.vpr",
                "fam/f2/y.vpr",
                "fam/unlisted/u.vpr",
                "off/b.vpr",
            ],
        );
        let config = config(
            &root,
            serde_json::json!([
                {"name": "flat"},
                {"name": "fam", "families": [{"name": "f1"}, {"name": "f2"}]},
                {"name": "off", "activated": false},
            ]),
        );

        let found: Vec<_> = Explorer::new(&config).map(Result::unwrap).collect();
        let summary: Vec<_> = found
            .iter()
            .map(|b| {
                (
                    b.suite.as_str(),
                    b.family.as_deref(),
                    b.test.as_str(),
                    b.rust.is_some(),
                    b.viper.is_some(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("flat", None, "a", false, true),
                ("fam", Some("f1"), "only_rust", true, false),
                ("fam", Some("f1"), "x", true, true),
                ("fam", Some("f1"), "z", false, true),
                ("fam", Some("f1"), "sub/deep", false, true),
                ("fam", Some("f1"), "sub/more/deeper", false, true),
                ("fam", Some("f2"), "y", false, true),
            ]
        );
        let x = &found[2];
        assert_eq!(x.rust.as_deref(), Some(root.join("fam/f1/x.rs").as_path()));
        assert_eq!(x.viper.as_deref(), Some(root.join("fam/f1/x.vpr").as_path()));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    #[ignore = "needs runner/suite.local.json and the suites"]
    fn explores_local_suites() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("suite.local.json");
        let config = SuiteConfig::new(path).unwrap();
        let mut counts = BTreeMap::<(String, bool, bool), usize>::new();
        for b in Explorer::new(&config) {
            let b = b.unwrap();
            *counts.entry((b.suite, b.rust.is_some(), b.viper.is_some())).or_default() += 1;
        }
        for ((suite, rust, viper), n) in &counts {
            println!("{suite:32} rust={rust:5} viper={viper:5} {n}");
        }
        assert!(counts.keys().any(|(s, r, v)| s == "scaling" && *r && *v));
    }

    #[test]
    fn missing_directory_is_an_error_then_continues() {
        let root = std::env::temp_dir().join(format!("explorer_missing_{}", std::process::id()));
        touch(&root, &["ok/a.vpr"]);
        let config = config(&root, serde_json::json!([{"name": "gone"}, {"name": "ok"}]));

        let results: Vec<_> = Explorer::new(&config).collect();
        assert_eq!(results.len(), 2);
        assert!(results[0].is_err());
        assert_eq!(results[1].as_ref().unwrap().test, "a");

        fs::remove_dir_all(&root).unwrap();
    }
}
