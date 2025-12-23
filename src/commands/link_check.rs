use std::path::{Path, PathBuf};

use crate::package::{LinkRule, VersionedLink};
use crate::runtime::{Runtime, is_path_under};

/// Result of checking a link's validity
#[derive(Debug, Clone)]
pub enum LinkStatus {
    /// Link exists and points to the expected location
    Valid,
    /// Link doesn't exist yet (will be created)
    NotExists,
    /// Link exists but points to a different location
    WrongTarget,
    /// Path exists but is not a symlink
    NotSymlink,
    /// Cannot resolve the symlink target
    Unresolvable,
}

impl LinkStatus {
    pub fn reason(&self) -> &'static str {
        match self {
            LinkStatus::Valid => "valid",
            LinkStatus::NotExists => "does not exist",
            LinkStatus::WrongTarget => "points to different location",
            LinkStatus::NotSymlink => "not a symlink",
            LinkStatus::Unresolvable => "cannot resolve target",
        }
    }

    pub fn is_valid(&self) -> bool {
        matches!(self, LinkStatus::Valid)
    }

    pub fn will_be_created(&self) -> bool {
        matches!(self, LinkStatus::NotExists)
    }
}

/// Checked link with its status
#[derive(Debug, Clone)]
pub struct CheckedLink {
    pub dest: PathBuf,
    pub status: LinkStatus,
    pub path: Option<String>,
}

/// Check if a symlink at `dest` points to somewhere under `expected_parent_dir`
pub fn check_link<R: Runtime>(runtime: &R, dest: &Path, expected_parent_dir: &Path) -> LinkStatus {
    if runtime.is_symlink(dest) {
        if let Ok(resolved_target) = runtime.resolve_link(dest) {
            if is_path_under(&resolved_target, expected_parent_dir) {
                LinkStatus::Valid
            } else {
                LinkStatus::WrongTarget
            }
        } else {
            LinkStatus::Unresolvable
        }
    } else if runtime.exists(dest) {
        LinkStatus::NotSymlink
    } else {
        LinkStatus::NotExists
    }
}

/// Check all links in a list and return categorized results
pub fn check_links<R: Runtime>(
    runtime: &R,
    links: &[LinkRule],
    expected_parent_dir: &Path,
) -> (Vec<CheckedLink>, Vec<CheckedLink>) {
    let mut valid = Vec::new();
    let mut invalid = Vec::new();

    for link in links {
        let status = check_link(runtime, &link.dest, expected_parent_dir);
        let checked = CheckedLink {
            dest: link.dest.clone(),
            status: status.clone(),
            path: link.path.clone(),
        };

        if status.is_valid() || status.will_be_created() {
            valid.push(checked);
        } else {
            invalid.push(checked);
        }
    }

    (valid, invalid)
}

/// Check all versioned links and return categorized results
pub fn check_versioned_links<R: Runtime>(
    runtime: &R,
    links: &[VersionedLink],
    expected_parent_dir: &Path,
) -> (Vec<CheckedLink>, Vec<CheckedLink>) {
    let mut valid = Vec::new();
    let mut invalid = Vec::new();

    for link in links {
        let status = check_link(runtime, &link.dest, expected_parent_dir);
        let checked = CheckedLink {
            dest: link.dest.clone(),
            status: status.clone(),
            path: link.path.clone(),
        };

        if status.is_valid() || status.will_be_created() {
            valid.push(checked);
        } else {
            invalid.push(checked);
        }
    }

    (valid, invalid)
}

/// Check versioned links for a specific version
pub fn check_versioned_links_for_version<R: Runtime>(
    runtime: &R,
    links: &[VersionedLink],
    version: &str,
    expected_parent_dir: &Path,
) -> (Vec<CheckedLink>, Vec<CheckedLink>) {
    let filtered: Vec<_> = links
        .iter()
        .filter(|l| l.version == version)
        .cloned()
        .collect();
    let as_link_rules: Vec<LinkRule> = filtered
        .iter()
        .map(|l| LinkRule {
            dest: l.dest.clone(),
            path: l.path.clone(),
        })
        .collect();
    check_links(runtime, &as_link_rules, expected_parent_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockall::predicate::*;

    #[test]
    fn test_check_link_valid() {
        let mut runtime = MockRuntime::new();
        let dest = PathBuf::from("/usr/local/bin/tool");
        let package_dir = PathBuf::from("/home/user/.ghri/owner/repo");
        let resolved = PathBuf::from("/home/user/.ghri/owner/repo/v1/tool");

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(dest.clone()))
            .returning(move |_| Ok(resolved.clone()));

        let status = check_link(&runtime, &dest, &package_dir);
        assert!(matches!(status, LinkStatus::Valid));
    }

    #[test]
    fn test_check_link_not_exists() {
        let mut runtime = MockRuntime::new();
        let dest = PathBuf::from("/usr/local/bin/tool");
        let package_dir = PathBuf::from("/home/user/.ghri/owner/repo");

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);

        let status = check_link(&runtime, &dest, &package_dir);
        assert!(matches!(status, LinkStatus::NotExists));
    }

    #[test]
    fn test_check_link_wrong_target() {
        let mut runtime = MockRuntime::new();
        let dest = PathBuf::from("/usr/local/bin/tool");
        let package_dir = PathBuf::from("/home/user/.ghri/owner/repo");
        let resolved = PathBuf::from("/home/user/.ghri/other/package/v1/tool");

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(dest.clone()))
            .returning(move |_| Ok(resolved.clone()));

        let status = check_link(&runtime, &dest, &package_dir);
        assert!(matches!(status, LinkStatus::WrongTarget));
    }

    #[test]
    fn test_check_link_not_symlink() {
        let mut runtime = MockRuntime::new();
        let dest = PathBuf::from("/usr/local/bin/tool");
        let package_dir = PathBuf::from("/home/user/.ghri/owner/repo");

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| true);

        let status = check_link(&runtime, &dest, &package_dir);
        assert!(matches!(status, LinkStatus::NotSymlink));
    }

    #[test]
    fn test_check_links_categorizes_correctly() {
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/home/user/.ghri/owner/repo");

        let valid_dest = PathBuf::from("/usr/local/bin/tool1");
        let invalid_dest = PathBuf::from("/usr/local/bin/tool2");

        // Valid link
        runtime
            .expect_is_symlink()
            .with(eq(valid_dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(valid_dest.clone()))
            .returning(|_| Ok(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool1")));

        // Invalid link (wrong target)
        runtime
            .expect_is_symlink()
            .with(eq(invalid_dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(invalid_dest.clone()))
            .returning(|_| Ok(PathBuf::from("/other/location/tool2")));

        let links = vec![
            LinkRule {
                dest: valid_dest,
                path: None,
            },
            LinkRule {
                dest: invalid_dest,
                path: None,
            },
        ];

        let (valid, invalid) = check_links(&runtime, &links, &package_dir);
        assert_eq!(valid.len(), 1);
        assert_eq!(invalid.len(), 1);
    }

    #[test]
    fn test_check_link_unresolvable() {
        let mut runtime = MockRuntime::new();
        let dest = PathBuf::from("/usr/local/bin/tool");
        let package_dir = PathBuf::from("/home/user/.ghri/owner/repo");

        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(dest.clone()))
            .returning(|_| Err(std::io::Error::new(std::io::ErrorKind::NotFound, "broken").into()));

        let status = check_link(&runtime, &dest, &package_dir);
        assert!(matches!(status, LinkStatus::Unresolvable));
    }

    #[test]
    fn test_link_status_reason() {
        assert_eq!(LinkStatus::Valid.reason(), "valid");
        assert_eq!(LinkStatus::NotExists.reason(), "does not exist");
        assert_eq!(
            LinkStatus::WrongTarget.reason(),
            "points to different location"
        );
        assert_eq!(LinkStatus::NotSymlink.reason(), "not a symlink");
        assert_eq!(LinkStatus::Unresolvable.reason(), "cannot resolve target");
    }

    #[test]
    fn test_link_status_is_valid() {
        assert!(LinkStatus::Valid.is_valid());
        assert!(!LinkStatus::NotExists.is_valid());
        assert!(!LinkStatus::WrongTarget.is_valid());
        assert!(!LinkStatus::NotSymlink.is_valid());
        assert!(!LinkStatus::Unresolvable.is_valid());
    }

    #[test]
    fn test_link_status_will_be_created() {
        assert!(!LinkStatus::Valid.will_be_created());
        assert!(LinkStatus::NotExists.will_be_created());
        assert!(!LinkStatus::WrongTarget.will_be_created());
        assert!(!LinkStatus::NotSymlink.will_be_created());
        assert!(!LinkStatus::Unresolvable.will_be_created());
    }

    #[test]
    fn test_check_versioned_links() {
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/home/user/.ghri/owner/repo");

        let valid_dest = PathBuf::from("/usr/local/bin/tool1");
        let invalid_dest = PathBuf::from("/usr/local/bin/tool2");

        // Valid link
        runtime
            .expect_is_symlink()
            .with(eq(valid_dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(valid_dest.clone()))
            .returning(|_| Ok(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool1")));

        // Invalid link (not a symlink)
        runtime
            .expect_is_symlink()
            .with(eq(invalid_dest.clone()))
            .returning(|_| false);
        runtime
            .expect_exists()
            .with(eq(invalid_dest.clone()))
            .returning(|_| true);

        let links = vec![
            VersionedLink {
                version: "v1".into(),
                dest: valid_dest,
                path: None,
            },
            VersionedLink {
                version: "v1".into(),
                dest: invalid_dest,
                path: None,
            },
        ];

        let (valid, invalid) = check_versioned_links(&runtime, &links, &package_dir);
        assert_eq!(valid.len(), 1);
        assert_eq!(invalid.len(), 1);
    }

    #[test]
    fn test_check_versioned_links_for_version() {
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/home/user/.ghri/owner/repo/v1");

        let v1_dest = PathBuf::from("/usr/local/bin/tool1");
        let v2_dest = PathBuf::from("/usr/local/bin/tool2");

        // Only v1 link should be checked
        runtime
            .expect_is_symlink()
            .with(eq(v1_dest.clone()))
            .returning(|_| true);
        runtime
            .expect_resolve_link()
            .with(eq(v1_dest.clone()))
            .returning(|_| Ok(PathBuf::from("/home/user/.ghri/owner/repo/v1/tool1")));

        let links = vec![
            VersionedLink {
                version: "v1".into(),
                dest: v1_dest,
                path: Some("bin/tool1".into()),
            },
            VersionedLink {
                version: "v2".into(),
                dest: v2_dest,
                path: None,
            }, // Should be filtered out
        ];

        let (valid, invalid) =
            check_versioned_links_for_version(&runtime, &links, "v1", &package_dir);
        assert_eq!(valid.len(), 1);
        assert_eq!(invalid.len(), 0);
        assert_eq!(valid[0].path, Some("bin/tool1".into()));
    }

    #[test]
    fn test_check_links_with_not_exists() {
        let mut runtime = MockRuntime::new();
        let package_dir = PathBuf::from("/home/user/.ghri/owner/repo");
        let dest = PathBuf::from("/usr/local/bin/tool");

        // Link doesn't exist yet
        runtime
            .expect_is_symlink()
            .with(eq(dest.clone()))
            .returning(|_| false);
        runtime
            .expect_exists()
            .with(eq(dest.clone()))
            .returning(|_| false);

        let links = vec![LinkRule {
            dest,
            path: Some("bin/tool".into()),
        }];

        let (valid, invalid) = check_links(&runtime, &links, &package_dir);
        // NotExists goes to valid (will be created)
        assert_eq!(valid.len(), 1);
        assert_eq!(invalid.len(), 0);
        assert!(valid[0].status.will_be_created());
        assert_eq!(valid[0].path, Some("bin/tool".into()));
    }
}
