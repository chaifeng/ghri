use anyhow::{Context, Result, bail};
use log::debug;
use std::path::Path;

use crate::runtime::Runtime;

/// Update the 'current' symlink to point to the specified version
#[tracing::instrument(skip(runtime, target_dir, _tag_name))]
pub fn update_current_symlink<R: Runtime>(
    runtime: &R,
    target_dir: &Path,
    _tag_name: &str,
) -> Result<()> {
    let current_link = target_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Failed to get parent directory"))?
        .join("current");

    let link_target = target_dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Failed to get target directory name"))?;

    let mut create_symlink = true;

    if runtime.exists(&current_link) {
        if !runtime.is_symlink(&current_link) {
            bail!("'current' exists but is not a symlink");
        }

        match runtime.read_link(&current_link) {
            Ok(target) => {
                // Normalize paths for comparison to handle minor differences like trailing slashes
                let existing_target_path = Path::new(&target).components().as_path();
                let new_target_path = Path::new(link_target).components().as_path();

                if existing_target_path == new_target_path {
                    debug!("'current' symlink already points to the correct version");
                    create_symlink = false;
                } else {
                    debug!(
                        "'current' symlink points to {:?}, but should point to {:?}. Updating...",
                        existing_target_path, new_target_path
                    );
                    runtime.remove_symlink(&current_link)?;
                }
            }
            Err(_) => {
                debug!("'current' symlink is unreadable, recreating...");
                runtime.remove_symlink(&current_link)?;
            }
        }
    }

    if create_symlink {
        runtime
            .symlink(Path::new(link_target), &current_link)
            .with_context(|| format!("Failed to update 'current' symlink to {:?}", target_dir))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use mockall::predicate::eq;
    use std::path::PathBuf;

    #[test]
    fn test_update_current_symlink_create_new() {
        let mut runtime = MockRuntime::new();
        let target_dir = PathBuf::from("/root/o/r/v1");
        let current_link = PathBuf::from("/root/o/r/current");

        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| false);

        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("v1")), eq(current_link.clone()))
            .returning(|_, _| Ok(()));

        update_current_symlink(&runtime, &target_dir, "v1").unwrap();
    }

    #[test]
    fn test_update_current_symlink_update_existing() {
        let mut runtime = MockRuntime::new();
        let target_dir = PathBuf::from("/root/o/r/v2");
        let current_link = PathBuf::from("/root/o/r/current");

        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1")));

        runtime
            .expect_remove_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(()));

        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("v2")), eq(current_link.clone()))
            .returning(|_, _| Ok(()));

        update_current_symlink(&runtime, &target_dir, "v2").unwrap();
    }

    #[test]
    fn test_update_current_symlink_fails_if_not_symlink() {
        let mut runtime = MockRuntime::new();
        let target_dir = PathBuf::from("/root/o/r/v1");
        let current_link = PathBuf::from("/root/o/r/current");

        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| false);

        let result = update_current_symlink(&runtime, &target_dir, "v1");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_current_symlink_idempotent() {
        let mut runtime = MockRuntime::new();
        let target_dir = PathBuf::from("/root/o/r/v1");
        let current_link = PathBuf::from("/root/o/r/current");

        // Exists, matches
        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1")));

        // Should NOT call remove_symlink or symlink
        let result = update_current_symlink(&runtime, &target_dir, "v1");
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_current_symlink_read_link_fail() {
        let mut runtime = MockRuntime::new();
        let target_dir = PathBuf::from("/root/o/r/v1");
        let current_link = PathBuf::from("/root/o/r/current");

        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Err(anyhow::anyhow!("fail")));

        // Should remove and recreate
        runtime
            .expect_remove_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(()));
        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("v1")), eq(current_link.clone()))
            .returning(|_, _| Ok(()));

        update_current_symlink(&runtime, &target_dir, "v1").unwrap();
    }

    #[test]
    fn test_update_current_symlink_no_op_if_already_correct() {
        let mut runtime = MockRuntime::new();
        runtime.expect_exists().returning(|_| true);
        runtime.expect_is_symlink().returning(|_| true);
        runtime
            .expect_read_link()
            .returning(|_| Ok(PathBuf::from("v1")));

        // No symlink calls
        update_current_symlink(&runtime, &PathBuf::from("/root/o/r/v1"), "v1").unwrap();
    }
}
