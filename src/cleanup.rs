use log::debug;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Tracks paths that need cleanup on interruption
#[derive(Default)]
pub struct CleanupContext {
    #[cfg(test)]
    pub paths: Vec<PathBuf>,
    #[cfg(not(test))]
    paths: Vec<PathBuf>,
}

impl CleanupContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a path to be cleaned up on interruption
    pub fn add(&mut self, path: PathBuf) {
        self.paths.push(path);
    }

    /// Remove a path from cleanup list (e.g., when operation succeeds)
    pub fn remove(&mut self, path: &Path) {
        self.paths.retain(|p| p != path);
    }

    /// Clean up all registered paths
    pub fn cleanup(&self) {
        for path in &self.paths {
            debug!("Cleaning up: {:?}", path);
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Type alias for shared cleanup context
pub type SharedCleanupContext = Arc<Mutex<CleanupContext>>;

/// Create a new shared cleanup context
pub fn new_shared() -> SharedCleanupContext {
    Arc::new(Mutex::new(CleanupContext::new()))
}

/// RAII guard that automatically removes a path from cleanup context when dropped
pub struct CleanupGuard {
    ctx: SharedCleanupContext,
    path: PathBuf,
}

impl CleanupGuard {
    /// Create a new cleanup guard and register the path
    pub fn new(ctx: SharedCleanupContext, path: PathBuf) -> Self {
        {
            let mut guard = ctx.lock().unwrap();
            guard.add(path.clone());
        }
        Self { ctx, path }
    }

    /// Mark the operation as successful, removing the path from cleanup
    pub fn success(self) {
        {
            let mut guard = self.ctx.lock().unwrap();
            guard.remove(&self.path);
        }
        // Don't run Drop since we've already removed the path
        std::mem::forget(self);
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        // Path remains in cleanup context if not explicitly marked as success
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_cleanup_context_add_remove() {
        let mut ctx = CleanupContext::new();
        let path = PathBuf::from("/tmp/test");

        ctx.add(path.clone());
        assert_eq!(ctx.paths.len(), 1);

        ctx.remove(&path);
        assert_eq!(ctx.paths.len(), 0);
    }

    #[test]
    fn test_cleanup_context_cleanup_files() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "test").unwrap();

        let mut ctx = CleanupContext::new();
        ctx.add(file_path.clone());

        assert!(file_path.exists());
        ctx.cleanup();
        assert!(!file_path.exists());
    }

    #[test]
    fn test_cleanup_context_cleanup_dirs() {
        let dir = tempdir().unwrap();
        let sub_dir = dir.path().join("subdir");
        fs::create_dir(&sub_dir).unwrap();
        fs::write(sub_dir.join("file.txt"), "test").unwrap();

        let mut ctx = CleanupContext::new();
        ctx.add(sub_dir.clone());

        assert!(sub_dir.exists());
        ctx.cleanup();
        assert!(!sub_dir.exists());
    }

    #[test]
    fn test_cleanup_guard_success() {
        let ctx = new_shared();
        let path = PathBuf::from("/tmp/test");

        {
            let guard = CleanupGuard::new(Arc::clone(&ctx), path.clone());
            assert_eq!(ctx.lock().unwrap().paths.len(), 1);
            guard.success();
        }

        assert_eq!(ctx.lock().unwrap().paths.len(), 0);
    }

    #[test]
    fn test_cleanup_guard_drop_without_success() {
        let ctx = new_shared();
        let path = PathBuf::from("/tmp/test");

        {
            let _guard = CleanupGuard::new(Arc::clone(&ctx), path.clone());
            assert_eq!(ctx.lock().unwrap().paths.len(), 1);
            // guard drops here without success()
        }

        // Path should remain in cleanup context
        assert_eq!(ctx.lock().unwrap().paths.len(), 1);
    }
}
