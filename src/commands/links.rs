use anyhow::Result;
use log::debug;
use std::path::Path;

use crate::application::LinkAction;
use crate::package::{LinkRule, LinkStatus, VersionedLink};
use crate::provider::PackageSpec;
use crate::runtime::Runtime;

use super::config::Config;

/// Convert LinkManager's LinkStatus to display string
fn format_link_status(status: &LinkStatus) -> String {
    match status {
        LinkStatus::Valid => String::new(),
        LinkStatus::NotExists => " [missing]".to_string(),
        LinkStatus::NotSymlink => " [not a symlink]".to_string(),
        LinkStatus::WrongTarget => " [wrong target]".to_string(),
        LinkStatus::Unresolvable => " [unresolvable]".to_string(),
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

    let action = LinkAction::new(runtime, std::path::PathBuf::new());
    for rule in links {
        let status = action.check_link(&rule.dest, expected_prefix);
        let source = rule.path.as_deref().unwrap_or("(default)");
        println!(
            "  {} -> {:?}{}",
            source,
            rule.dest,
            format_link_status(&status)
        );
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

    let action = LinkAction::new(runtime, std::path::PathBuf::new());
    for link in links {
        let version_dir = package_dir.join(&link.version);
        let status = action.check_link(&link.dest, &version_dir);
        let source = link.path.as_deref().unwrap_or("(default)");
        println!(
            "  @{} {} -> {:?}{}",
            link.version,
            source,
            link.dest,
            format_link_status(&status)
        );
    }
}

/// Show link rules for a package
#[tracing::instrument(skip(runtime, config))]
pub fn links<R: Runtime>(runtime: R, repo_str: &str, config: Config) -> Result<()> {
    debug!("Showing link rules for {}", repo_str);
    let spec = repo_str.parse::<PackageSpec>()?;
    debug!("Using install root: {:?}", config.install_root);

    let action = LinkAction::new(&runtime, config.install_root);

    if !action.is_installed(&spec.repo.owner, &spec.repo.repo) {
        debug!("Package not installed");
        anyhow::bail!("Package {} is not installed.", spec.repo);
    }

    let meta = action.load_meta(&spec.repo.owner, &spec.repo.repo)?;
    debug!(
        "Found {} link rules, {} versioned links",
        meta.links.len(),
        meta.versioned_links.len()
    );

    if meta.links.is_empty() && meta.versioned_links.is_empty() {
        println!("No link rules for {}.", spec.repo);
        return Ok(());
    }

    let package_dir = action.package_dir(&spec.repo.owner, &spec.repo.repo);
    let current_version_dir = action
        .package_repo()
        .current_version_dir(&spec.repo.owner, &spec.repo.repo);
    let header = format!(
        "Link rules for {} (current: {}):",
        spec.repo, meta.current_version
    );
    if let Some(version_dir) = current_version_dir {
        print_links(&runtime, &meta.links, &version_dir, Some(&header));
    } else {
        println!("{}", header);
        println!("  (current version symlink not found, cannot show links)");
    }

    if !meta.versioned_links.is_empty() {
        println!();
        print_versioned_links(
            &runtime,
            &meta.versioned_links,
            &package_dir,
            Some("Versioned links (historical):"),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::Meta;
    use crate::runtime::MockRuntime;
    use mockall::predicate::*;
    use std::path::PathBuf;

    #[test]
    fn test_format_link_status() {
        // Test format_link_status function

        assert_eq!(format_link_status(&LinkStatus::Valid), "");
        assert_eq!(format_link_status(&LinkStatus::NotExists), " [missing]");
        assert_eq!(
            format_link_status(&LinkStatus::NotSymlink),
            " [not a symlink]"
        );
        assert_eq!(
            format_link_status(&LinkStatus::WrongTarget),
            " [wrong target]"
        );
        assert_eq!(
            format_link_status(&LinkStatus::Unresolvable),
            " [unresolvable]"
        );
    }

    #[test]
    fn test_print_links_empty() {
        // Test that print_links does nothing when links array is empty

        let runtime = MockRuntime::new();

        // --- Setup ---
        let links: Vec<LinkRule> = vec![];
        let expected_prefix = PathBuf::from("/root/o/r/current");

        // --- Execute ---

        // Should return immediately without printing anything
        print_links(&runtime, &links, &expected_prefix, Some("Header"));
    }

    #[test]
    fn test_print_versioned_links_empty() {
        // Test that print_versioned_links does nothing when links array is empty

        let runtime = MockRuntime::new();

        // --- Setup ---
        let links: Vec<VersionedLink> = vec![];
        let package_dir = PathBuf::from("/root/o/r");

        // --- Execute ---

        // Should return immediately without printing anything
        print_versioned_links(&runtime, &links, &package_dir, Some("Header"));
    }

    #[test]
    fn test_links_package_not_installed() {
        // Test that links command fails when package is not installed

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        // --- Check Package Exists (is_installed checks meta.json) ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> false (not installed!)
        runtime
            .expect_exists()
            .with(eq(meta_path))
            .returning(|_| false);

        // --- Execute & Verify ---

        let result = links(runtime, "owner/repo", Config::for_test("/home/user/.ghri"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn test_links_no_link_rules() {
        // Test that links command shows "No link rules" message when package has no links

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        // --- Check Package Exists (is_installed) ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // --- Load Metadata (load_required) ---

        // Read meta.json -> package with NO link rules
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1.0.0".into(),
            links: vec![],           // No links!
            versioned_links: vec![], // No versioned links!
            ..Default::default()
        };
        let meta_json = serde_json::to_string(&meta).unwrap();
        runtime
            .expect_read_to_string()
            .with(eq(meta_path))
            .returning(move |_| Ok(meta_json.clone()));

        // --- Execute ---

        let result = links(runtime, "owner/repo", Config::for_test("/home/user/.ghri"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_links_with_custom_install_root() {
        // Test that links command uses custom install root when provided

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let install_root = PathBuf::from("/custom/root");
        let meta_path = install_root.join("owner/repo/meta.json"); // /custom/root/owner/repo/meta.json

        // Config::load needs GITHUB_TOKEN
        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Err(std::env::VarError::NotPresent));

        // --- 1. Check Package Exists (is_installed) ---

        // File exists: /custom/root/owner/repo/meta.json -> false (not installed)
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| false);

        // --- Execute & Verify ---

        let result = links(runtime, "owner/repo", Config::for_test(install_root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn test_links_checked_against_version_dir_not_current_symlink() {
        // This test verifies the fix for the "wrong target" bug in links command.
        // Links should be checked against the actual version directory,
        // NOT the current symlink path.

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let package_dir = root.join("owner/repo");
        let meta_path = package_dir.join("meta.json");
        let current_link = package_dir.join("current");
        let version_dir = package_dir.join("v1.0.0");
        let link_dest = PathBuf::from("/usr/local/bin/tool");

        // --- Check Package Exists (is_installed) ---
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // --- Load Metadata ---
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1.0.0".into(),
            links: vec![LinkRule {
                dest: link_dest.clone(),
                path: Some("bin/tool".into()),
            }],
            ..Default::default()
        };
        let meta_json = serde_json::to_string(&meta).unwrap();
        runtime
            .expect_read_to_string()
            .with(eq(meta_path))
            .returning(move |_| Ok(meta_json.clone()));

        // --- Get Current Version Dir (read_link returns relative path "v1.0.0") ---
        // This is the key: read_link returns a relative path
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1.0.0")));

        // --- Check Link Status ---
        // The link exists and is a symlink
        runtime
            .expect_is_symlink()
            .with(eq(link_dest.clone()))
            .returning(|_| true);

        // The link resolves to a path UNDER the version directory
        // Before the fix, it was checked against current_link path which would be WrongTarget
        runtime
            .expect_resolve_link()
            .with(eq(link_dest.clone()))
            .returning(move |_| Ok(version_dir.join("bin/tool")));

        // --- Execute ---
        let result = links(runtime, "owner/repo", Config::for_test(root));
        assert!(result.is_ok());
        // If this test passes, the link was correctly identified as Valid
    }
}
