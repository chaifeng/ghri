//! Link rule for creating external symlinks to installed packages.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A link rule that describes how to create an external symlink
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct LinkRule {
    /// The destination path for the symlink
    pub dest: PathBuf,
    /// Relative path within version directory to link (e.g., "bin/tool")
    /// If None, uses default behavior (single file or version directory)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_rule_default() {
        let rule = LinkRule::default();
        assert_eq!(rule.dest, PathBuf::new());
        assert_eq!(rule.path, None);
    }

    #[test]
    fn test_link_rule_serialize() {
        let rule = LinkRule {
            dest: PathBuf::from("/usr/local/bin/tool"),
            path: Some("bin/tool".to_string()),
        };
        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains("/usr/local/bin/tool"));
        assert!(json.contains("bin/tool"));
    }

    #[test]
    fn test_link_rule_serialize_no_path() {
        let rule = LinkRule {
            dest: PathBuf::from("/usr/local/bin/tool"),
            path: None,
        };
        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains("/usr/local/bin/tool"));
        assert!(!json.contains("path")); // skip_serializing_if
    }

    #[test]
    fn test_link_rule_deserialize() {
        let json = r#"{"dest": "/usr/local/bin/tool", "path": "bin/tool"}"#;
        let rule: LinkRule = serde_json::from_str(json).unwrap();
        assert_eq!(rule.dest, PathBuf::from("/usr/local/bin/tool"));
        assert_eq!(rule.path, Some("bin/tool".to_string()));
    }

    #[test]
    fn test_link_rule_deserialize_no_path() {
        let json = r#"{"dest": "/usr/local/bin/tool"}"#;
        let rule: LinkRule = serde_json::from_str(json).unwrap();
        assert_eq!(rule.dest, PathBuf::from("/usr/local/bin/tool"));
        assert_eq!(rule.path, None);
    }
}
