use anyhow::Result;
use log::debug;
use std::path::{Path, PathBuf};

use crate::{
    github::RepoSpec,
    package::{LinkManager, LinkRule, LinkStatus, PackageRepository, VersionedLink},
    runtime::Runtime,
};

use super::paths::default_install_root;

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

    let link_manager = LinkManager::new(runtime);
    for rule in links {
        let status = link_manager.check_link(&rule.dest, expected_prefix);
        let source = rule.path.as_deref().unwrap_or("(default)");
        println!("  {} -> {:?}{}", source, rule.dest, format_link_status(&status));
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

    let link_manager = LinkManager::new(runtime);
    for link in links {
        let version_dir = package_dir.join(&link.version);
        let status = link_manager.check_link(&link.dest, &version_dir);
        let source = link.path.as_deref().unwrap_or("(default)");
        println!(
            "  @{} {} -> {:?}{}",
            link.version, source, link.dest, format_link_status(&status)
        );
    }
}

/// Show link rules for a package
#[tracing::instrument(skip(runtime, install_root))]
pub fn links<R: Runtime>(runtime: R, repo_str: &str, install_root: Option<PathBuf>) -> Result<()> {
    debug!("Showing link rules for {}", repo_str);
    let spec = repo_str.parse::<RepoSpec>()?;
    let root = match install_root {
        Some(path) => path,
        None => default_install_root(&runtime)?,
    };
    debug!("Using install root: {:?}", root);

    let pkg_repo = PackageRepository::new(&runtime, root);
    
    if !pkg_repo.is_installed(&spec.repo.owner, &spec.repo.repo) {
        debug!("Package not installed");
        anyhow::bail!("Package {} is not installed.", spec.repo);
    }

    let meta = pkg_repo.load_required(&spec.repo.owner, &spec.repo.repo)?;
    debug!(
        "Found {} link rules, {} versioned links",
        meta.links.len(),
        meta.versioned_links.len()
    );

    if meta.links.is_empty() && meta.versioned_links.is_empty() {
        println!("No link rules for {}.", spec.repo);
        return Ok(());
    }

    let package_dir = pkg_repo.package_dir(&spec.repo.owner, &spec.repo.repo);
    let current_dir = pkg_repo.current_link(&spec.repo.owner, &spec.repo.repo);
    let header = format!(
        "Link rules for {} (current: {}):",
        spec.repo, meta.current_version
    );
    print_links(&runtime, &meta.links, &current_dir, Some(&header));

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

    #[test]
    fn test_format_link_status() {
        // Test format_link_status function

        assert_eq!(format_link_status(&LinkStatus::Valid), "");
        assert_eq!(format_link_status(&LinkStatus::NotExists), " [missing]");
        assert_eq!(format_link_status(&LinkStatus::NotSymlink), " [not a symlink]");
        assert_eq!(format_link_status(&LinkStatus::WrongTarget), " [wrong target]");
        assert_eq!(format_link_status(&LinkStatus::Unresolvable), " [unresolvable]");
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

        // --- 1. Get Default Install Root ---

        runtime.expect_is_privileged().returning(|| false);
        runtime
            .expect_home_dir()
            .returning(|| Some(PathBuf::from("/home/user")));

        // --- 2. Check Package Exists (is_installed checks meta.json) ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> false (not installed!)
        runtime
            .expect_exists()
            .with(eq(meta_path))
            .returning(|_| false);

        // --- Execute & Verify ---

        let result = links(runtime, "owner/repo", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn test_links_no_link_rules() {
        // Test that links command shows "No link rules" message when package has no links

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        // --- 1. Get Default Install Root ---

        runtime.expect_is_privileged().returning(|| false);
        runtime
            .expect_home_dir()
            .returning(|| Some(PathBuf::from("/home/user")));

        // --- 2. Check Package Exists (is_installed) ---

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // --- 3. Load Metadata (load_required) ---

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

        let result = links(runtime, "owner/repo", None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_links_with_custom_install_root() {
        // Test that links command uses custom install root when provided

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let install_root = PathBuf::from("/custom/root");
        let meta_path = install_root.join("owner/repo/meta.json"); // /custom/root/owner/repo/meta.json

        // --- 1. Check Package Exists (is_installed) ---

        // File exists: /custom/root/owner/repo/meta.json -> false (not installed)
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| false);

        // --- Execute & Verify ---

        let result = links(runtime, "owner/repo", Some(install_root));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not installed"));
    }
}
