//! Workspace configuration discovery and parsing.
//!
//! Looks for `.async-rust-lsp.toml` next to the workspace's `Cargo.toml`.
//! The format is intentionally minimal — one section per rule:
//!
//! ```toml
//! [rules.cancel-unsafe-in-select]
//! # Project-local function/method names that wrap cancel-unsafe tokio
//! # primitives. Add wrappers here so the rule flags them in `select!`
//! # arms even though it can't follow function bodies across files.
//! extra = ["recv_typed_frame", "send_typed_frame"]
//! ```

use serde::Deserialize;
use std::path::{Path, PathBuf};

const CONFIG_FILENAME: &str = ".async-rust-lsp.toml";

/// Top-level config file shape. Unknown keys are ignored so adding new
/// rules later doesn't break older clients.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub rules: Rules,
}

#[derive(Debug, Default, Deserialize)]
pub struct Rules {
    #[serde(default, rename = "cancel-unsafe-in-select")]
    pub cancel_unsafe_in_select: CancelUnsafeInSelect,
}

#[derive(Debug, Default, Deserialize)]
pub struct CancelUnsafeInSelect {
    /// Project-local cancel-unsafe function/method names. Combined with
    /// the rule's built-in tokio-primitives list at check time.
    #[serde(default)]
    pub extra: Vec<String>,
}

impl Config {
    /// Walk up from `start` looking for `.async-rust-lsp.toml`. Returns
    /// the parsed config and the directory it lives in. If no file is
    /// found, returns the default config and `start` (so callers can
    /// still cache the negative result).
    pub fn discover_from(start: &Path) -> (Config, PathBuf) {
        let mut dir = start;
        loop {
            let candidate = dir.join(CONFIG_FILENAME);
            if candidate.is_file() {
                let cfg = match std::fs::read_to_string(&candidate) {
                    Ok(text) => toml::from_str::<Config>(&text).unwrap_or_else(|e| {
                        // Log via tracing — never panic on malformed user config.
                        tracing::warn!(
                            "failed to parse {}: {}; using defaults",
                            candidate.display(),
                            e
                        );
                        Config::default()
                    }),
                    Err(e) => {
                        tracing::warn!("failed to read {}: {}", candidate.display(), e);
                        Config::default()
                    }
                };
                return (cfg, dir.to_path_buf());
            }
            match dir.parent() {
                Some(p) => dir = p,
                None => break,
            }
        }
        (Config::default(), start.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn empty_config_parses() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.rules.cancel_unsafe_in_select.extra.is_empty());
    }

    #[test]
    fn config_with_extras_parses() {
        let toml = r#"
[rules.cancel-unsafe-in-select]
extra = ["recv_typed_frame", "send_typed_frame"]
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.rules.cancel_unsafe_in_select.extra,
            vec![
                "recv_typed_frame".to_string(),
                "send_typed_frame".to_string()
            ]
        );
    }

    #[test]
    fn unknown_top_level_keys_are_ignored() {
        let toml = r#"
unrelated = "stuff"

[rules.cancel-unsafe-in-select]
extra = ["foo"]

[rules.future_rule_we_havent_added]
key = "value"
"#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.rules.cancel_unsafe_in_select.extra, vec!["foo"]);
    }

    #[test]
    fn discover_walks_up_to_find_file() {
        let tmp = tempdir();
        let root = tmp.path();
        fs::write(
            root.join(CONFIG_FILENAME),
            r#"[rules.cancel-unsafe-in-select]
extra = ["wrapper_a"]
"#,
        )
        .unwrap();
        let nested = root.join("a/b/c");
        fs::create_dir_all(&nested).unwrap();

        let (cfg, found_dir) = Config::discover_from(&nested);
        assert_eq!(cfg.rules.cancel_unsafe_in_select.extra, vec!["wrapper_a"]);
        assert_eq!(
            found_dir.canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
    }

    #[test]
    fn discover_with_no_file_returns_default() {
        let tmp = tempdir();
        let nested = tmp.path().join("a/b");
        fs::create_dir_all(&nested).unwrap();
        let (cfg, _dir) = Config::discover_from(&nested);
        assert!(cfg.rules.cancel_unsafe_in_select.extra.is_empty());
    }

    /// Lightweight tempdir without pulling in a tempfile dep. Cleaned
    /// up by the OS later; tests are short-lived.
    fn tempdir() -> TempDir {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "async-rust-lsp-test-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        std::fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }

    fn rand_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
