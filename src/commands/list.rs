use anyhow::Result;
use log::debug;

use crate::{package::PackageRepository, runtime::Runtime};

use super::config::Config;

/// List all installed packages
#[tracing::instrument(skip(runtime, config))]
pub fn list<R: Runtime>(runtime: R, config: Config) -> Result<()> {
    debug!("Listing packages from {:?}", config.install_root);

    let repo = PackageRepository::new(&runtime, config.install_root);
    let packages = repo.find_all_with_meta()?;

    if packages.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    debug!("Found {} package(s)", packages.len());

    for (_meta_path, meta) in packages {
        let version = if meta.current_version.is_empty() {
            "(unknown)".to_string()
        } else {
            meta.current_version.clone()
        };
        println!("{} {}", meta.name, version);
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
    fn test_list_no_packages() {
        // Test that list shows "No packages installed" when directory is empty

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");

        // --- 1. Get Default Install Root ---

        runtime.expect_is_privileged().returning(|| false);
        runtime
            .expect_home_dir()
            .returning(|| Some(PathBuf::from("/home/user")));
        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Err(std::env::VarError::NotPresent));

        // --- 2. Find All Packages ---

        // Directory exists: /home/user/.ghri -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // Read dir /home/user/.ghri -> empty (no packages)
        runtime
            .expect_read_dir()
            .with(eq(root))
            .returning(|_| Ok(vec![]));

        // --- Execute & Verify ---

        let result = list(runtime, Config::for_test("/home/user/.ghri"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_with_packages() {
        // Test that list displays installed packages with their versions

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let owner_dir = root.join("owner"); // /home/user/.ghri/owner
        let repo_dir = owner_dir.join("repo"); // /home/user/.ghri/owner/repo
        let meta_path = repo_dir.join("meta.json"); // /home/user/.ghri/owner/repo/meta.json

        // --- 1. Get Default Install Root ---

        runtime.expect_is_privileged().returning(|| false);
        runtime
            .expect_home_dir()
            .returning(|| Some(PathBuf::from("/home/user")));
        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Err(std::env::VarError::NotPresent));

        // --- 2. Find All Packages ---

        // Directory exists: /home/user/.ghri -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // Read dir /home/user/.ghri -> [/home/user/.ghri/owner]
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(move |_| Ok(vec![owner_dir.clone()]));

        // Is dir: /home/user/.ghri/owner -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner")))
            .returning(|_| true);

        // Read dir /home/user/.ghri/owner -> [/home/user/.ghri/owner/repo]
        runtime
            .expect_read_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner")))
            .returning(|_| Ok(vec![PathBuf::from("/home/user/.ghri/owner/repo")]));

        // Is dir: /home/user/.ghri/owner/repo -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo")))
            .returning(|_| true);

        // File exists: /home/user/.ghri/owner/repo/meta.json -> true
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/meta.json")))
            .returning(|_| true);

        // --- 3. Load Package Metadata ---

        // Read meta.json -> package with version v1.0.0
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "v1.0.0".into(),
            ..Default::default()
        };
        let meta_json = serde_json::to_string(&meta).unwrap();
        runtime
            .expect_read_to_string()
            .with(eq(meta_path))
            .returning(move |_| Ok(meta_json.clone()));

        // --- Execute ---

        let result = list(runtime, Config::for_test("/home/user/.ghri"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_with_custom_install_root() {
        // Test that list uses custom install root when provided

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let root = PathBuf::from("/custom/root");

        // Config::load needs GITHUB_TOKEN
        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Err(std::env::VarError::NotPresent));

        // --- 1. Find All Packages (using custom root) ---

        // Directory exists: /custom/root -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // Read dir /custom/root -> empty
        runtime
            .expect_read_dir()
            .with(eq(root))
            .returning(|_| Ok(vec![]));

        // --- Execute ---

        let result = list(runtime, Config::for_test("/custom/root"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_with_empty_version() {
        // Test that list shows "(unknown)" when current_version is empty

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let owner_dir = root.join("owner");
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");
        let current_link = PathBuf::from("/home/user/.ghri/owner/repo/current");

        // --- 1. Get Default Install Root ---

        runtime.expect_is_privileged().returning(|| false);
        runtime
            .expect_home_dir()
            .returning(|| Some(PathBuf::from("/home/user")));
        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Err(std::env::VarError::NotPresent));

        // --- 2. Find All Packages ---

        // Directory exists: /home/user/.ghri -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // Read dir /home/user/.ghri -> [owner]
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(move |_| Ok(vec![owner_dir.clone()]));

        // Is dir: /home/user/.ghri/owner -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner")))
            .returning(|_| true);

        // Read dir /home/user/.ghri/owner -> [repo]
        runtime
            .expect_read_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner")))
            .returning(|_| Ok(vec![PathBuf::from("/home/user/.ghri/owner/repo")]));

        // Is dir: /home/user/.ghri/owner/repo -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo")))
            .returning(|_| true);

        // File exists: meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // --- 3. Load Package Metadata (empty version) ---

        // Read meta.json -> package with EMPTY current_version
        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "".into(), // Empty version!
            ..Default::default()
        };
        let meta_json = serde_json::to_string(&meta).unwrap();
        runtime
            .expect_read_to_string()
            .with(eq(meta_path))
            .returning(move |_| Ok(meta_json.clone()));

        // --- 4. Try to Read Current Symlink (for version fallback) ---

        // Read symlink /home/user/.ghri/owner/repo/current -> v1.0.0
        runtime
            .expect_read_link()
            .with(eq(current_link))
            .returning(|_| Ok(PathBuf::from("v1.0.0")));

        // --- Execute ---

        let result = list(runtime, Config::for_test("/home/user/.ghri"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_meta_load_error() {
        // Test that list continues gracefully when meta.json is invalid

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let root = PathBuf::from("/home/user/.ghri");
        let owner_dir = root.join("owner");
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        // --- 1. Get Default Install Root ---

        runtime.expect_is_privileged().returning(|| false);
        runtime
            .expect_home_dir()
            .returning(|| Some(PathBuf::from("/home/user")));
        runtime
            .expect_env_var()
            .with(eq("GITHUB_TOKEN"))
            .returning(|_| Err(std::env::VarError::NotPresent));

        // --- 2. Find All Packages ---

        // Directory exists: /home/user/.ghri -> true
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);

        // Read dir /home/user/.ghri -> [owner]
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(move |_| Ok(vec![owner_dir.clone()]));

        // Is dir: /home/user/.ghri/owner -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner")))
            .returning(|_| true);

        // Read dir /home/user/.ghri/owner -> [repo]
        runtime
            .expect_read_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner")))
            .returning(|_| Ok(vec![PathBuf::from("/home/user/.ghri/owner/repo")]));

        // Is dir: /home/user/.ghri/owner/repo -> true
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo")))
            .returning(|_| true);

        // File exists: meta.json -> true
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // --- 3. Load Package Metadata (INVALID JSON) ---

        // Read meta.json -> INVALID JSON (triggers parse error)
        runtime
            .expect_read_to_string()
            .with(eq(meta_path))
            .returning(|_| Ok("invalid json".to_string()));

        // --- Execute & Verify ---

        // Should still succeed, just skip the failed package
        let result = list(runtime, Config::for_test("/home/user/.ghri"));
        assert!(result.is_ok());
    }
}
