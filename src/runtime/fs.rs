//! File system operations (read, write, directory, permissions).

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use super::RealRuntime;

impl RealRuntime {
    #[tracing::instrument(skip(self, contents))]
    pub(crate) fn write_impl(&self, path: &Path, contents: &[u8]) -> Result<()> {
        fs::write(path, contents).context("Failed to write to file")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn read_to_string_impl(&self, path: &Path) -> Result<String> {
        fs::read_to_string(path).context("Failed to read file to string")
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn rename_impl(&self, from: &Path, to: &Path) -> Result<()> {
        fs::rename(from, to).context("Failed to rename file")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn copy_impl(&self, from: &Path, to: &Path) -> Result<u64> {
        fs::copy(from, to).context("Failed to copy file")
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn create_dir_all_impl(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path).context("Failed to create directory")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn remove_file_impl(&self, path: &Path) -> Result<()> {
        fs::remove_file(path).context("Failed to remove file")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn remove_dir_impl(&self, path: &Path) -> Result<()> {
        fs::remove_dir(path).context("Failed to remove directory")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn exists_impl(&self, path: &Path) -> bool {
        path.exists()
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn read_dir_impl(&self, path: &Path) -> Result<Vec<PathBuf>> {
        fs::read_dir(path)?.map(|entry| Ok(entry?.path())).collect()
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn create_file_impl(&self, path: &Path) -> Result<Box<dyn std::io::Write + Send>> {
        let file = std::fs::File::create(path).context("Failed to create file")?;
        Ok(Box::new(file))
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn open_impl(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>> {
        let file = std::fs::File::open(path).context("Failed to open file")?;
        Ok(Box::new(file))
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn remove_dir_all_impl(&self, path: &Path) -> Result<()> {
        fs::remove_dir_all(path).context("Failed to remove directory and its contents")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn is_dir_impl(&self, path: &Path) -> bool {
        path.is_dir()
    }

    #[tracing::instrument(skip(self))]
    pub(crate) fn set_permissions_impl(&self, path: &Path, mode: u32) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = fs::Permissions::from_mode(mode);
            fs::set_permissions(path, permissions).context("Failed to set permissions")?;
        }
        #[cfg(not(unix))]
        {
            let _ = (path, mode); // Suppress unused warnings on non-Unix
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::{RealRuntime, Runtime};
    use std::io::{Read, Write};
    use tempfile::tempdir;

    #[test]
    fn test_real_runtime_file_ops() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        // Test write
        runtime.write(&file_path, b"hello").unwrap();
        assert!(runtime.exists(&file_path));

        // Test read_to_string
        let content = runtime.read_to_string(&file_path).unwrap();
        assert_eq!(content, "hello");

        // Test copy
        let copy_path = dir.path().join("copy.txt");
        runtime.copy(&file_path, &copy_path).unwrap();
        assert!(runtime.exists(&copy_path));

        // Test rename
        let new_path = dir.path().join("renamed.txt");
        runtime.rename(&file_path, &new_path).unwrap();
        assert!(!runtime.exists(&file_path));
        assert!(runtime.exists(&new_path));

        // Test remove_file
        runtime.remove_file(&new_path).unwrap();
        assert!(!runtime.exists(&new_path));

        // Clean up
        runtime.remove_file(&copy_path).unwrap();
    }

    #[test]
    fn test_real_runtime_dir_ops() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        let sub_dir = dir.path().join("sub/nested");

        // Test create_dir_all
        runtime.create_dir_all(&sub_dir).unwrap();
        assert!(runtime.exists(&sub_dir));
        assert!(runtime.is_dir(&sub_dir));

        // Test read_dir
        let parent = dir.path().join("sub");
        let entries = runtime.read_dir(&parent).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].ends_with("nested"));

        // Test remove_dir
        runtime.remove_dir(&sub_dir).unwrap();
        assert!(!runtime.exists(&sub_dir));

        // Test remove_dir_all
        runtime.create_dir_all(&sub_dir).unwrap();
        runtime.remove_dir_all(&parent).unwrap();
        assert!(!runtime.exists(&parent));
    }

    #[test]
    fn test_real_runtime_create_file_and_open() {
        let runtime = RealRuntime;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("stream.txt");

        // Test create_file
        {
            let mut writer = runtime.create_file(&file_path).unwrap();
            writer.write_all(b"streamed content").unwrap();
        }

        // Test open
        {
            let mut reader = runtime.open(&file_path).unwrap();
            let mut content = String::new();
            reader.read_to_string(&mut content).unwrap();
            assert_eq!(content, "streamed content");
        }
    }

    #[test]
    fn test_real_runtime_errors() {
        let runtime = RealRuntime;

        // Test read non-existent file
        let result = runtime.read_to_string(std::path::Path::new("/nonexistent/path/file.txt"));
        assert!(result.is_err());

        // Test remove non-existent file
        let result = runtime.remove_file(std::path::Path::new("/nonexistent/path/file.txt"));
        assert!(result.is_err());
    }
}
