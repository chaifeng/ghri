use anyhow::Result;
use log::debug;
use std::path::PathBuf;

use crate::{
    package::{Meta, find_all_packages},
    runtime::Runtime,
};

use super::paths::default_install_root;

/// List all installed packages
#[tracing::instrument(skip(runtime, install_root))]
pub fn list<R: Runtime>(runtime: R, install_root: Option<PathBuf>) -> Result<()> {
    let root = match install_root {
        Some(path) => path,
        None => default_install_root(&runtime)?,
    };

    debug!("Listing packages from {:?}", root);

    let meta_files = find_all_packages(&runtime, &root)?;
    if meta_files.is_empty() {
        println!("No packages installed.");
        return Ok(());
    }

    debug!("Found {} package(s)", meta_files.len());

    for meta_path in meta_files {
        match Meta::load(&runtime, &meta_path) {
            Ok(meta) => {
                let version = if meta.current_version.is_empty() {
                    "(unknown)".to_string()
                } else {
                    meta.current_version.clone()
                };
                println!("{} {}", meta.name, version);
            }
            Err(e) => {
                debug!("Failed to load meta from {:?}: {}", meta_path, e);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockall::predicate::*;

    #[test]
    fn test_list_no_packages() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/home/user/.ghri");

        runtime.expect_is_privileged().returning(|| false);
        runtime.expect_home_dir().returning(|| Some(PathBuf::from("/home/user")));
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root))
            .returning(|_| Ok(vec![]));

        let result = list(runtime, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_with_packages() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/home/user/.ghri");
        let owner_dir = root.join("owner");
        let repo_dir = owner_dir.join("repo");
        let meta_path = repo_dir.join("meta.json");

        runtime.expect_is_privileged().returning(|| false);
        runtime.expect_home_dir().returning(|| Some(PathBuf::from("/home/user")));
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(move |_| Ok(vec![owner_dir.clone()]));
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner")))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner")))
            .returning(|_| Ok(vec![PathBuf::from("/home/user/.ghri/owner/repo")]));
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo")))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/meta.json")))
            .returning(|_| true);

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

        let result = list(runtime, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_with_custom_install_root() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/custom/root");

        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root))
            .returning(|_| Ok(vec![]));

        let result = list(runtime, Some(PathBuf::from("/custom/root")));
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_with_empty_version() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/home/user/.ghri");
        let owner_dir = root.join("owner");
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        runtime.expect_is_privileged().returning(|| false);
        runtime.expect_home_dir().returning(|| Some(PathBuf::from("/home/user")));
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(move |_| Ok(vec![owner_dir.clone()]));
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner")))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner")))
            .returning(|_| Ok(vec![PathBuf::from("/home/user/.ghri/owner/repo")]));
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo")))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        let meta = Meta {
            name: "owner/repo".into(),
            current_version: "".into(), // Empty version
            ..Default::default()
        };
        let meta_json = serde_json::to_string(&meta).unwrap();
        runtime
            .expect_read_to_string()
            .with(eq(meta_path))
            .returning(move |_| Ok(meta_json.clone()));

        // When current_version is empty, Meta::load will try to read the current symlink
        runtime
            .expect_read_link()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo/current")))
            .returning(|_| Ok(PathBuf::from("v1.0.0")));

        let result = list(runtime, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_meta_load_error() {
        let mut runtime = MockRuntime::new();
        let root = PathBuf::from("/home/user/.ghri");
        let owner_dir = root.join("owner");
        let meta_path = PathBuf::from("/home/user/.ghri/owner/repo/meta.json");

        runtime.expect_is_privileged().returning(|| false);
        runtime.expect_home_dir().returning(|| Some(PathBuf::from("/home/user")));
        runtime
            .expect_exists()
            .with(eq(root.clone()))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(root.clone()))
            .returning(move |_| Ok(vec![owner_dir.clone()]));
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner")))
            .returning(|_| true);
        runtime
            .expect_read_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner")))
            .returning(|_| Ok(vec![PathBuf::from("/home/user/.ghri/owner/repo")]));
        runtime
            .expect_is_dir()
            .with(eq(PathBuf::from("/home/user/.ghri/owner/repo")))
            .returning(|_| true);
        runtime
            .expect_exists()
            .with(eq(meta_path.clone()))
            .returning(|_| true);

        // Return invalid JSON to trigger error
        runtime
            .expect_read_to_string()
            .with(eq(meta_path))
            .returning(|_| Ok("invalid json".to_string()));

        // Should still succeed, just skip the failed package
        let result = list(runtime, None);
        assert!(result.is_ok());
    }
}
