use base64::prelude::{BASE64_STANDARD, Engine};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub type RelPath = Vec<u8>;

pub fn join(dir: &[u8], name: &[u8]) -> RelPath {
    if dir.is_empty() {
        return name.to_vec();
    }
    let mut p = Vec::with_capacity(dir.len() + 1 + name.len());
    p.extend_from_slice(dir);
    p.push(b'/');
    p.extend_from_slice(name);
    p
}

pub fn parent(p: &[u8]) -> &[u8] {
    match p.iter().rposition(|&c| c == b'/') {
        Some(i) => &p[..i],
        None => b"",
    }
}

pub fn file_name(p: &[u8]) -> &[u8] {
    match p.iter().rposition(|&c| c == b'/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

pub fn is_under(p: &[u8], dir: &[u8]) -> bool {
    if dir.is_empty() {
        return !p.is_empty();
    }
    p.len() > dir.len() && p.starts_with(dir) && p[dir.len()] == b'/'
}

pub fn ancestors(p: &[u8]) -> Vec<&[u8]> {
    p.iter().enumerate().filter(|&(_, &c)| c == b'/').map(|(i, _)| &p[..i]).collect()
}

pub fn is_valid(p: &[u8]) -> bool {
    !p.is_empty() && !p.contains(&0) && p.split(|&c| c == b'/').all(|c| !c.is_empty() && c != b"." && c != b"..")
}

pub fn resolve_redirect(p: &[u8], redirect: &[u8]) -> RelPath {
    match redirect.strip_prefix(b"/") {
        Some(abs) => abs.to_vec(),
        None => join(parent(p), redirect),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(description = "A path in API messages: a string, or `{\"base64\": ...}` when it is not UTF-8.")]
pub enum PathName {
    Utf8(String),
    #[schemars(title = "Base64Path")]
    Base64 {
        base64: String,
    },
}

impl PathName {
    pub fn new(p: &[u8]) -> Self {
        match std::str::from_utf8(p) {
            Ok(s) => PathName::Utf8(s.to_string()),
            Err(_) => PathName::Base64 { base64: b64_encode(p) },
        }
    }

    pub fn to_bytes(&self) -> Option<RelPath> {
        match self {
            PathName::Utf8(s) => Some(s.as_bytes().to_vec()),
            PathName::Base64 { base64 } => b64_decode(base64),
        }
    }
}

pub fn b64_encode(data: &[u8]) -> String {
    BASE64_STANDARD.encode(data)
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    BASE64_STANDARD.decode(s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert!(is_under(b"a/b", b"a"));
        assert!(!is_under(b"ab", b"a"));
        assert!(!is_under(b"a", b"a"));
        assert!(is_under(b"a", b""));
        assert!(is_valid(b"a/b"));
        for bad in [&b""[..], b"/a", b"a/", b"a//b", b"a/./b", b"../a", b"a\0b"] {
            assert!(!is_valid(bad), "{:?}", bad);
        }
        assert_eq!(resolve_redirect(b"x/new", b"old"), b"x/old");
        assert_eq!(resolve_redirect(b"x/new", b"/y/old"), b"y/old");
    }

    #[test]
    fn base64_roundtrip() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"\xff\x00\n\xfe"] {
            assert_eq!(b64_decode(&b64_encode(data)).unwrap(), data);
        }
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        let bad = b"a\xffb".to_vec();
        assert_eq!(PathName::new(&bad).to_bytes().unwrap(), bad);
        assert_eq!(serde_json::to_string(&PathName::new(b"x/y")).unwrap(), r#""x/y""#);
        assert_eq!(serde_json::from_str::<PathName>(r#"{"base64":"YQ=="}"#).unwrap().to_bytes().unwrap(), b"a");
    }
}
