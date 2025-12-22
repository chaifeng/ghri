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
        // Test creating a new 'current' symlink when it doesn't exist

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let target_dir = PathBuf::from("/root/o/r/v1");           // Version directory
        let current_link = PathBuf::from("/root/o/r/current");    // Symlink to create

        // --- 1. Check if Symlink Exists ---

        // File exists: /root/o/r/current -> false (doesn't exist)
        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| false);

        // --- 2. Create Symlink ---

        // Create symlink: /root/o/r/current -> v1
        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("v1")), eq(current_link.clone()))
            .returning(|_, _| Ok(()));

        // --- Execute ---
        update_current_symlink(&runtime, &target_dir, "v1").unwrap();
    }

    #[test]
    fn test_update_current_symlink_update_existing() {
        // Test updating 'current' symlink from v1 to v2

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let target_dir = PathBuf::from("/root/o/r/v2");           // New version directory
        let current_link = PathBuf::from("/root/o/r/current");    // Symlink to update

        // --- 1. Check if Symlink Exists ---

        // File exists: /root/o/r/current -> true
        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // Is symlink: /root/o/r/current -> true
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // --- 2. Read Current Target ---

        // Read link: /root/o/r/current -> v1 (old version)
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1")));

        // --- 3. Remove Old and Create New Symlink ---

        // Remove old symlink: /root/o/r/current
        runtime
            .expect_remove_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(()));

        // Create new symlink: /root/o/r/current -> v2
        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("v2")), eq(current_link.clone()))
            .returning(|_, _| Ok(()));

        // --- Execute ---
        update_current_symlink(&runtime, &target_dir, "v2").unwrap();
    }

    #[test]
    fn test_update_current_symlink_fails_if_not_symlink() {
        // Test that update fails if 'current' exists but is not a symlink (e.g., regular file)

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let target_dir = PathBuf::from("/root/o/r/v1");
        let current_link = PathBuf::from("/root/o/r/current");

        // --- 1. Check if Symlink Exists ---

        // File exists: /root/o/r/current -> true
        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // Is symlink: /root/o/r/current -> false (it's a regular file!)
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| false);

        // --- Execute & Verify ---

        // Should fail because 'current' is not a symlink
        let result = update_current_symlink(&runtime, &target_dir, "v1");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_current_symlink_idempotent() {
        // Test that update is idempotent - no changes when symlink already points to correct version

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let target_dir = PathBuf::from("/root/o/r/v1");
        let current_link = PathBuf::from("/root/o/r/current");

        // --- 1. Check if Symlink Exists ---

        // File exists: /root/o/r/current -> true
        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // Is symlink: /root/o/r/current -> true
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // --- 2. Read Current Target ---

        // Read link: /root/o/r/current -> v1 (already correct!)
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(PathBuf::from("v1")));

        // (No remove_symlink or symlink calls - already correct)

        // --- Execute & Verify ---

        // Should succeed without making any changes
        let result = update_current_symlink(&runtime, &target_dir, "v1");
        assert!(result.is_ok());
    }

    #[test]
    fn test_update_current_symlink_read_link_fail() {
        // Test that symlink is recreated when read_link fails (corrupted symlink)

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let target_dir = PathBuf::from("/root/o/r/v1");
        let current_link = PathBuf::from("/root/o/r/current");

        // --- 1. Check if Symlink Exists ---

        // File exists: /root/o/r/current -> true
        runtime
            .expect_exists()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // Is symlink: /root/o/r/current -> true
        runtime
            .expect_is_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| true);

        // --- 2. Try to Read Current Target (FAILS) ---

        // Read link: /root/o/r/current -> ERROR (corrupted/unreadable)
        runtime
            .expect_read_link()
            .with(eq(current_link.clone()))
            .returning(|_| Err(anyhow::anyhow!("fail")));

        // --- 3. Remove and Recreate Symlink ---

        // Remove corrupted symlink: /root/o/r/current
        runtime
            .expect_remove_symlink()
            .with(eq(current_link.clone()))
            .returning(|_| Ok(()));

        // Create new symlink: /root/o/r/current -> v1
        runtime
            .expect_symlink()
            .with(eq(PathBuf::from("v1")), eq(current_link.clone()))
            .returning(|_, _| Ok(()));

        // --- Execute ---
        update_current_symlink(&runtime, &target_dir, "v1").unwrap();
    }

    #[test]
    fn test_update_current_symlink_no_op_if_already_correct() {
        // Test that no symlink operations occur when target is already correct (variant)

        let mut runtime = MockRuntime::new();

        // --- Setup Paths ---
        let target_dir = PathBuf::from("/root/o/r/v1");

        // --- 1. Check Existing Symlink ---

        // File exists -> true, is symlink -> true, points to v1 -> already correct
        runtime.expect_exists().returning(|_| true);
        runtime.expect_is_symlink().returning(|_| true);
        runtime
            .expect_read_link()
            .returning(|_| Ok(PathBuf::from("v1")));

        // (No remove_symlink or symlink calls expected)

        // --- Execute ---
        update_current_symlink(&runtime, &target_dir, "v1").unwrap();
    }
}
