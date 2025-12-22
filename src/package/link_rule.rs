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

/// A versioned link record for tracking historical version links
/// These are links created with explicit version specifiers (e.g., owner/repo@v1.0.0)
/// They are not updated on install/update, only displayed and cleaned up on version removal
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct VersionedLink {
    /// The version this link was created for
    pub version: String,
    /// The destination path for the symlink
    pub dest: PathBuf,
    /// Relative path within version directory to link (e.g., "bin/tool")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_rule_default() {
        // Test that LinkRule::default() creates empty values

        let rule = LinkRule::default();

        assert_eq!(rule.dest, PathBuf::new());
        assert_eq!(rule.path, None);
    }

    #[test]
    fn test_link_rule_serialize() {
        // Test serializing LinkRule with path to JSON

        let rule = LinkRule {
            dest: PathBuf::from("/usr/local/bin/tool"),
            path: Some("bin/tool".to_string()),
        };

        let json = serde_json::to_string(&rule).unwrap();

        // JSON should contain both dest and path
        assert!(json.contains("/usr/local/bin/tool"));
        assert!(json.contains("bin/tool"));
    }

    #[test]
    fn test_link_rule_serialize_no_path() {
        // Test that path field is omitted from JSON when None (skip_serializing_if)

        let rule = LinkRule {
            dest: PathBuf::from("/usr/local/bin/tool"),
            path: None,
        };

        let json = serde_json::to_string(&rule).unwrap();

        // JSON should contain dest but NOT path (skip_serializing_if)
        assert!(json.contains("/usr/local/bin/tool"));
        assert!(!json.contains("path"));
    }

    #[test]
    fn test_link_rule_deserialize() {
        // Test deserializing LinkRule with path from JSON

        let json = r#"{"dest": "/usr/local/bin/tool", "path": "bin/tool"}"#;

        let rule: LinkRule = serde_json::from_str(json).unwrap();

        assert_eq!(rule.dest, PathBuf::from("/usr/local/bin/tool"));
        assert_eq!(rule.path, Some("bin/tool".to_string()));
    }

    #[test]
    fn test_link_rule_deserialize_no_path() {
        // Test deserializing LinkRule without path (defaults to None)

        let json = r#"{"dest": "/usr/local/bin/tool"}"#;

        let rule: LinkRule = serde_json::from_str(json).unwrap();

        assert_eq!(rule.dest, PathBuf::from("/usr/local/bin/tool"));
        assert_eq!(rule.path, None);
    }

    #[test]
    fn test_versioned_link_default() {
        // Test that VersionedLink::default() creates empty values

        let link = VersionedLink::default();

        assert_eq!(link.version, "");
        assert_eq!(link.dest, PathBuf::new());
        assert_eq!(link.path, None);
    }

    #[test]
    fn test_versioned_link_serialize() {
        // Test serializing VersionedLink to JSON

        let link = VersionedLink {
            version: "v1.0.0".to_string(),
            dest: PathBuf::from("/usr/local/bin/tool"),
            path: Some("bin/tool".to_string()),
        };

        let json = serde_json::to_string(&link).unwrap();

        // JSON should contain version, dest, and path
        assert!(json.contains("v1.0.0"));
        assert!(json.contains("/usr/local/bin/tool"));
        assert!(json.contains("bin/tool"));
    }

    #[test]
    fn test_versioned_link_deserialize() {
        // Test deserializing VersionedLink from JSON

        let json = r#"{"version": "v1.0.0", "dest": "/usr/local/bin/tool", "path": "bin/tool"}"#;

        let link: VersionedLink = serde_json::from_str(json).unwrap();

        assert_eq!(link.version, "v1.0.0");
        assert_eq!(link.dest, PathBuf::from("/usr/local/bin/tool"));
        assert_eq!(link.path, Some("bin/tool".to_string()));
    }
}
