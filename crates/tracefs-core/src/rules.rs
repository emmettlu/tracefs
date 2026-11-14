use std::borrow::Cow;
use std::path::Path;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

pub const IGNORE_FILE: &str = ".tracefsignore";
pub const MERGE_FILE: &str = ".tracefsmerge";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Strategy {
    Ours,
    Theirs,
    Newer,
    Union,
    Unit(String),
}

impl Strategy {
    fn parse(s: &str) -> Option<Strategy> {
        Some(match s {
            "ours" => Strategy::Ours,
            "theirs" => Strategy::Theirs,
            "newer" => Strategy::Newer,
            "union" => Strategy::Union,
            _ => Strategy::Unit(s.strip_prefix("unit=").filter(|n| !n.is_empty())?.to_string()),
        })
    }
}

#[derive(Default)]
pub struct Rules {
    ignore: Option<Gitignore>,
    merge: Vec<(Gitignore, Option<Strategy>)>,
}

#[cfg(unix)]
fn path(p: &[u8]) -> Cow<'_, Path> {
    use std::os::unix::ffi::OsStrExt;
    Cow::Borrowed(Path::new(std::ffi::OsStr::from_bytes(p)))
}

#[cfg(not(unix))]
fn path(p: &[u8]) -> Cow<'_, Path> {
    match String::from_utf8_lossy(p) {
        Cow::Borrowed(s) => Cow::Borrowed(Path::new(s)),
        Cow::Owned(s) => Cow::Owned(s.into()),
    }
}

impl Rules {
    pub fn parse(ignore: Option<&[u8]>, merge: Option<&[u8]>) -> (Rules, Vec<String>) {
        let mut warnings = Vec::new();
        let mut rules = Rules::default();
        if let Some(text) = ignore {
            let mut b = GitignoreBuilder::new("");
            for (n, line) in String::from_utf8_lossy(text).lines().enumerate() {
                if let Err(e) = b.add_line(None, line) {
                    warnings.push(format!("{IGNORE_FILE}:{}: {e}", n + 1));
                }
            }
            match b.build() {
                Ok(g) if !g.is_empty() => rules.ignore = Some(g),
                Ok(_) => {}
                Err(e) => warnings.push(format!("{IGNORE_FILE}: {e}")),
            }
        }
        if let Some(text) = merge {
            for (n, line) in String::from_utf8_lossy(text).lines().enumerate() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let mut words = line.split_whitespace();
                let (Some(pattern), strategy, None) = (words.next(), words.next(), words.next()) else {
                    warnings.push(format!("{MERGE_FILE}:{}: expected `<pattern> <strategy>`", n + 1));
                    continue;
                };
                let strategy = match (pattern.starts_with('!'), strategy.map(Strategy::parse)) {
                    (true, None) => None,
                    (false, Some(Some(s))) => Some(s),
                    _ => {
                        warnings.push(format!("{MERGE_FILE}:{}: expected ours, theirs, newer, union or unit=<name>", n + 1));
                        continue;
                    }
                };
                let mut b = GitignoreBuilder::new("");
                let built = b
                    .add_line(None, pattern.strip_prefix('!').unwrap_or(pattern))
                    .map_err(|e| e.to_string())
                    .and_then(|b| b.build().map_err(|e| e.to_string()));
                match built {
                    Ok(g) => rules.merge.push((g, strategy)),
                    Err(e) => warnings.push(format!("{MERGE_FILE}:{}: {e}", n + 1)),
                }
            }
        }
        (rules, warnings)
    }

    pub fn is_empty(&self) -> bool {
        self.ignore.is_none() && self.merge.is_empty()
    }

    pub fn ignored(&self, p: &[u8], is_dir: bool) -> bool {
        match &self.ignore {
            Some(g) if !p.is_empty() => g.matched_path_or_any_parents(path(p), is_dir).is_ignore(),
            _ => false,
        }
    }

    pub fn strategy(&self, p: &[u8], is_dir: bool) -> Option<&Strategy> {
        if p.is_empty() {
            return None;
        }
        let (_, s) = self.merge.iter().rev().find(|(g, _)| g.matched_path_or_any_parents(path(p), is_dir).is_ignore())?;
        s.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_rules() {
        let (r, w) = Rules::parse(Some(b"target/\n*.o\n!keep.o\n# comment\n/node_modules\n"), None);
        assert!(w.is_empty(), "{w:?}");
        assert!(r.ignored(b"target", true));
        assert!(r.ignored(b"target/debug/x", false));
        assert!(r.ignored(b"sub/target/y", false));
        assert!(!r.ignored(b"target", false), "a file named target is not a directory");
        assert!(r.ignored(b"a/b.o", false));
        assert!(!r.ignored(b"keep.o", false));
        assert!(r.ignored(b"node_modules/x", false));
        assert!(!r.ignored(b"sub/node_modules/x", false), "anchored");
        assert!(!r.ignored(b"", true));
        assert!(!Rules::default().ignored(b"target/x", false));
    }

    #[test]
    fn merge_rules() {
        let text = b"*.lock newer\nCargo.lock unit=cargo\nCargo.toml unit=cargo\ndist/ theirs\n!dist/keep\nCHANGELOG.md union\n.env ours\nbad\nx nope\n";
        let (r, w) = Rules::parse(None, Some(text));
        assert_eq!(w.len(), 2, "{w:?}");
        assert_eq!(r.strategy(b"yarn.lock", false), Some(&Strategy::Newer));
        assert_eq!(r.strategy(b"Cargo.lock", false), Some(&Strategy::Unit("cargo".into())), "the last match wins");
        assert_eq!(r.strategy(b"crates/a/Cargo.toml", false), Some(&Strategy::Unit("cargo".into())));
        assert_eq!(r.strategy(b"dist/app.js", false), Some(&Strategy::Theirs));
        assert_eq!(r.strategy(b"dist/keep", false), None);
        assert_eq!(r.strategy(b"CHANGELOG.md", false), Some(&Strategy::Union));
        assert_eq!(r.strategy(b".env", false), Some(&Strategy::Ours));
        assert_eq!(r.strategy(b"src/main.rs", false), None);
        assert!(!r.ignored(b"dist/app.js", false));
    }

    #[test]
    fn non_utf8_paths() {
        let (r, _) = Rules::parse(Some(b"*.tmp\n"), None);
        assert!(r.ignored(b"a\xff.tmp", false));
        assert!(!r.ignored(b"a\xff", false));
    }
}
