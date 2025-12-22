use anyhow::Result;
use log::debug;
use std::path::{Path, PathBuf};

use crate::{
    github::RepoSpec,
    package::{LinkRule, Meta, VersionedLink},
    runtime::Runtime,
};

use super::paths::default_install_root;

/// Link status for display
#[derive(Debug, PartialEq)]
enum LinkStatus {
    /// Link exists and points to the expected target
    Ok,
    /// Link path does not exist
    Missing,
    /// Path exists but is not a symlink
    NotSymlink,
    /// Link exists but points to a different target
    WrongTarget(PathBuf),
}

impl std::fmt::Display for LinkStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkStatus::Ok => write!(f, ""),
            LinkStatus::Missing => write!(f, " [missing]"),
            LinkStatus::NotSymlink => write!(f, " [not a symlink]"),
            LinkStatus::WrongTarget(target) => write!(f, " [wrong target: {}]", target.display()),
        }
    }
}

/// Check the status of a symlink
fn check_link_status<R: Runtime>(
    runtime: &R,
    link_dest: &Path,
    expected_prefix: &Path,
) -> LinkStatus {
    if !runtime.exists(link_dest) && !runtime.is_symlink(link_dest) {
        return LinkStatus::Missing;
    }

    if !runtime.is_symlink(link_dest) {
        return LinkStatus::NotSymlink;
    }

    match runtime.read_link(link_dest) {
        Ok(target) => {
            // Resolve relative paths
            let resolved = if target.is_relative() {
                link_dest.parent().unwrap_or(Path::new(".")).join(&target)
            } else {
                target.clone()
            };

            // Canonicalize for accurate comparison
            let canonicalized = std::fs::canonicalize(&resolved).unwrap_or(resolved);
            let canonicalized_prefix = std::fs::canonicalize(expected_prefix)
                .unwrap_or_else(|_| expected_prefix.to_path_buf());

            // Check if target is under expected prefix
            let target_components: Vec<_> = canonicalized.components().collect();
            let prefix_components: Vec<_> = canonicalized_prefix.components().collect();

            let is_under_prefix = prefix_components.len() <= target_components.len()
                && prefix_components
                    .iter()
                    .zip(target_components.iter())
                    .all(|(p, t)| p == t);

            if is_under_prefix {
                LinkStatus::Ok
            } else {
                LinkStatus::WrongTarget(canonicalized)
            }
        }
        Err(_) => LinkStatus::Missing,
    }
}

/// Format and print link rules with status
pub(crate) fn print_links<R: Runtime>(
    runtime: &R,
    links: &[LinkRule],
    expected_prefix: &Path,
    header: Option<&str>,
) {
    if links.is_empty() {
        return;
    }

    if let Some(h) = header {
        println!("{}", h);
    }

    for rule in links {
        let status = check_link_status(runtime, &rule.dest, expected_prefix);
        let source = rule.path.as_deref().unwrap_or("(default)");
        println!("  {} -> {:?}{}", source, rule.dest, status);
    }
}

/// Format and print versioned links with status
pub(crate) fn print_versioned_links<R: Runtime>(
    runtime: &R,
    links: &[VersionedLink],
    package_dir: &Path,
    header: Option<&str>,
) {
    if links.is_empty() {
        return;
    }

    if let Some(h) = header {
        println!("{}", h);
    }

    for link in links {
        let version_dir = package_dir.join(&link.version);
        let status = check_link_status(runtime, &link.dest, &version_dir);
        let source = link.path.as_deref().unwrap_or("(default)");
        println!("  @{} {} -> {:?}{}", link.version, source, link.dest, status);
    }
}

/// Show link rules for a package
#[tracing::instrument(skip(runtime, install_root))]
pub fn links<R: Runtime>(
    runtime: R,
    repo_str: &str,
    install_root: Option<PathBuf>,
) -> Result<()> {
    debug!("Showing link rules for {}", repo_str);
    let spec = repo_str.parse::<RepoSpec>()?;
    let root = match install_root {
        Some(path) => path,
        None => default_install_root(&runtime)?,
    };
    debug!("Using install root: {:?}", root);

    let package_dir = root.join(&spec.repo.owner).join(&spec.repo.repo);
    let meta_path = package_dir.join("meta.json");
    debug!("Loading meta from {:?}", meta_path);

    if !runtime.exists(&meta_path) {
        debug!("Meta file not found");
        anyhow::bail!(
            "Package {} is not installed.",
            spec.repo
        );
    }

    let meta = Meta::load(&runtime, &meta_path)?;
    debug!("Found {} link rules, {} versioned links", meta.links.len(), meta.versioned_links.len());

    if meta.links.is_empty() && meta.versioned_links.is_empty() {
        println!("No link rules for {}.", spec.repo);
        return Ok(());
    }

    let current_dir = package_dir.join("current");
    let header = format!("Link rules for {} (current: {}):", spec.repo, meta.current_version);
    print_links(&runtime, &meta.links, &current_dir, Some(&header));

    if !meta.versioned_links.is_empty() {
        println!();
        print_versioned_links(&runtime, &meta.versioned_links, &package_dir, Some("Versioned links (historical):"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockall::predicate::*;

    #[test]
    fn test_link_status_display() {
        assert_eq!(format!("{}", LinkStatus::Ok), "");
        assert_eq!(format!("{}", LinkStatus::Missing), " [missing]");
        assert_eq!(format!("{}", LinkStatus::NotSymlink), " [not a symlink]");
        assert_eq!(
            format!("{}", LinkStatus::WrongTarget(PathBuf::from("/other/path"))),
            " [wrong target: /other/path]"
        );
    }

    #[test]
    fn test_check_link_status_missing() {
        let mut runtime = MockRuntime::new();
        let link_dest = PathBuf::from("/usr/local/bin/tool");
        let expected_prefix = PathBuf::from("/root/o/r/current");

        runtime
            .expect_exists()
            .with(eq(link_dest.clone()))
            .returning(|_| false);
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| false);

        let status = check_link_status(&runtime, &link_dest, &expected_prefix);
        assert_eq!(status, LinkStatus::Missing);
    }

    #[test]
    fn test_check_link_status_not_symlink() {
        let mut runtime = MockRuntime::new();
        let link_dest = PathBuf::from("/usr/local/bin/tool");
        let expected_prefix = PathBuf::from("/root/o/r/current");

        // First check: exists returns true
        runtime
            .expect_exists()
            .with(eq(link_dest.clone()))
            .returning(|_| true);
        // First is_symlink check in the first condition
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| false);

        let status = check_link_status(&runtime, &link_dest, &expected_prefix);
        assert_eq!(status, LinkStatus::NotSymlink);
    }

    #[test]
    fn test_check_link_status_read_link_error() {
        let mut runtime = MockRuntime::new();
        let link_dest = PathBuf::from("/usr/local/bin/tool");
        let expected_prefix = PathBuf::from("/root/o/r/current");

        runtime
            .expect_exists()
            .with(eq(link_dest.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(link_dest.clone()))
            .returning(|_| Err(anyhow::anyhow!("not found")));

        let status = check_link_status(&runtime, &link_dest, &expected_prefix);
        assert_eq!(status, LinkStatus::Missing);
    }

    #[test]
    fn test_print_links_empty() {
        let runtime = MockRuntime::new();
        let links: Vec<LinkRule> = vec![];
        let expected_prefix = PathBuf::from("/root/o/r/current");

        // Should return without printing anything
        print_links(&runtime, &links, &expected_prefix, Some("Header"));
    }

    #[test]
    fn test_print_versioned_links_empty() {
        let runtime = MockRuntime::new();
        let links: Vec<VersionedLink> = vec![];
        let package_dir = PathBuf::from("/root/o/r");

        // Should return without printing anything
        print_versioned_links(&runtime, &links, &package_dir, Some("Header"));
    }

    #[test]
    fn test_links_package_not_installed() {
        let mut runtime = MockRuntime::new();

        runtime.expect_is_privileged().returning(|| false);
        runtime.expect_home_dir().returning(|| Some(PathBuf::from("/home/user")));
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/meta.json")))
            .returning(|_| false);

        let result = links(runtime, "owner/repo", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn test_links_no_link_rules() {
        let mut runtime = MockRuntime::new();
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        runtime.expect_is_privileged().returning(|| false);
        runtime.expect_home_dir().returning(|| Some(PathBuf::from("/home/user")));
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1.0.0".into(),
            links: vec![],
            versioned_links: vec![],
            ..Default::default()
        };
        let meta_json = serde_json::to_string(&meta).unwrap();
        runtime
            .expect_read_to_string()
            .with(eq(meta_path))
            .returning(move |_| Ok(meta_json.clone()));

        let result = links(runtime, "owner/repo", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_links_with_custom_install_root() {
        let mut runtime = MockRuntime::new();
        let install_root = PathBuf::from("/custom/root");
        let meta_path = install_root.join("owner/repo/meta.json");

        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| false);

        let result = links(runtime, "owner/repo", Some(install_root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }
}
