use anyhow::{Context, Result};
use async_trait::async_trait;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait Runtime: Send + Sync {
    // Environment
    fn env_var(&self, key: &str) -> Result<String, env::VarError>;

    // File System
    fn write(&self, path: &Path, contents: &[u8]) -> Result<()>;
    fn read_to_string(&self, path: &Path) -> Result<String>;
    fn rename(&self, from: &Path, to: &Path) -> Result<()>;
    fn create_dir_all(&self, path: &Path) -> Result<()>;
    fn remove_file(&self, path: &Path) -> Result<()>;
    fn remove_dir(&self, path: &Path) -> Result<()>;
    fn remove_symlink(&self, path: &Path) -> Result<()>;
    fn exists(&self, path: &Path) -> bool;
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>>;
    fn symlink(&self, original: &Path, link: &Path) -> Result<()>;
    fn read_link(&self, path: &Path) -> Result<PathBuf>;
    fn is_symlink(&self, path: &Path) -> bool;
    fn create_file(&self, path: &Path) -> Result<Box<dyn std::io::Write + Send>>;
    fn open(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>>;
    fn remove_dir_all(&self, path: &Path) -> Result<()>;
    fn is_dir(&self, path: &Path) -> bool;

    // Directories
    fn home_dir(&self) -> Option<PathBuf>;
    fn config_dir(&self) -> Option<PathBuf>;
}

pub struct RealRuntime;

#[async_trait]
impl Runtime for RealRuntime {
    #[tracing::instrument(skip(self))]
    fn env_var(&self, key: &str) -> Result<String, env::VarError> {
        env::var(key)
    }

    #[tracing::instrument(skip(self, contents))]
    fn write(&self, path: &Path, contents: &[u8]) -> Result<()> {
        fs::write(path, contents).context("Failed to write to file")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn read_to_string(&self, path: &Path) -> Result<String> {
        fs::read_to_string(path).context("Failed to read file to string")
    }

    #[tracing::instrument(skip(self))]
    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        fs::rename(from, to).context("Failed to rename file")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path).context("Failed to create directory")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn remove_file(&self, path: &Path) -> Result<()> {
        fs::remove_file(path).context("Failed to remove file")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn remove_dir(&self, path: &Path) -> Result<()> {
        fs::remove_dir(path).context("Failed to remove directory")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn remove_symlink(&self, path: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            fs::remove_file(path).context("Failed to remove symlink")?;
        }
        #[cfg(windows)]
        {
            // On Windows, removing a symlink requires remove_dir for a directory symlink
            // and remove_file for a file symlink. We try to remove it as a directory
            // first, and if that fails, we try to remove it as a file.
            fs::remove_dir(path)
                .or_else(|_| fs::remove_file(path))
                .context("Failed to remove symlink")?;
        }
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    #[tracing::instrument(skip(self))]
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        fs::read_dir(path)?.map(|entry| Ok(entry?.path())).collect()
    }

    #[tracing::instrument(skip(self))]
    fn symlink(&self, original: &Path, link: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink as unix_symlink;
            unix_symlink(original, link).context("Failed to create symlink")?;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::{symlink_dir, symlink_file};

            // If `original` is a relative path, `is_dir()` would check it against the
            // current working directory. We need to check it relative to the directory
            // where the symlink will be created.
            let target_path = if original.is_absolute() {
                original.to_path_buf()
            } else {
                link.parent()
                    .context("Failed to get parent directory for symlink")?
                    .join(original)
            };

            if target_path.is_dir() {
                symlink_dir(original, link).context("Failed to create directory symlink")?;
            } else {
                symlink_file(original, link).context("Failed to create file symlink")?;
            }
        }
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn read_link(&self, path: &Path) -> Result<PathBuf> {
        fs::read_link(path).context("Failed to read symlink")
    }

    #[tracing::instrument(skip(self))]
    fn is_symlink(&self, path: &Path) -> bool {
        fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }

    #[tracing::instrument(skip(self))]
    fn create_file(&self, path: &Path) -> Result<Box<dyn std::io::Write + Send>> {
        let file = std::fs::File::create(path).context("Failed to create file")?;
        Ok(Box::new(file))
    }

    #[tracing::instrument(skip(self))]
    fn open(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>> {
        let file = std::fs::File::open(path).context("Failed to open file")?;
        Ok(Box::new(file))
    }

    #[tracing::instrument(skip(self))]
    fn remove_dir_all(&self, path: &Path) -> Result<()> {
        fs::remove_dir_all(path).context("Failed to remove directory and its contents")?;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    #[tracing::instrument(skip(self))]
    fn home_dir(&self) -> Option<PathBuf> {
        dirs::home_dir()
    }

    #[tracing::instrument(skip(self))]
    fn config_dir(&self) -> Option<PathBuf> {
        dirs::config_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use tempfile::tempdir;

    #[test]
    fn test_real_runtime_file_ops() {
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        // Write
        rt.write(&file_path, b"hello").unwrap();
        assert!(rt.exists(&file_path));

        // Read
        let content = rt.read_to_string(&file_path).unwrap();
        assert_eq!(content, "hello");

        // Open
        let mut reader = rt.open(&file_path).unwrap();
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "hello");

        // Rename
        let new_path = dir.path().join("test_new.txt");
        rt.rename(&file_path, &new_path).unwrap();
        assert!(!rt.exists(&file_path));
        assert!(rt.exists(&new_path));

        // Create file using write stream
        let file_path2 = dir.path().join("test2.txt");
        {
            let mut writer = rt.create_file(&file_path2).unwrap();
            writer.write_all(b"world").unwrap();
        }
        assert_eq!(rt.read_to_string(&file_path2).unwrap(), "world");

        // Remove
        rt.remove_file(&new_path).unwrap();
        assert!(!rt.exists(&new_path));
    }

    #[test]
    fn test_real_runtime_dir_ops() {
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let sub_dir = dir.path().join("a/b/c");

        // Create
        rt.create_dir_all(&sub_dir).unwrap();
        assert!(rt.exists(&sub_dir));
        assert!(rt.is_dir(&sub_dir));

        // Read dir
        let parent = sub_dir.parent().unwrap();
        let entries = rt.read_dir(parent).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], sub_dir);

        // Remove dir
        rt.remove_dir(&sub_dir).unwrap();
        assert!(!rt.exists(&sub_dir));

        // Remove dir all
        let sub_dir_full = dir.path().join("x/y/z");
        rt.create_dir_all(&sub_dir_full).unwrap();
        rt.write(&sub_dir_full.join("file.txt"), b"data").unwrap();
        rt.remove_dir_all(&dir.path().join("x")).unwrap();
        assert!(!rt.exists(&dir.path().join("x")));
    }

    #[test]
    fn test_real_runtime_symlink_ops() {
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let target = dir.path().join("target");
        let link = dir.path().join("link");

        // Create target (must be dir for Windows compatibility in some cases)
        rt.create_dir_all(&target).unwrap();

        // Symlink
        rt.symlink(&target, &link).unwrap();
        assert!(rt.exists(&link));
        assert!(rt.is_symlink(&link));

        // Read link
        let read_target = rt.read_link(&link).unwrap();
        // Note: read_link might return relative or absolute depends on how it was created
        // In our case we passed absolute path
        assert_eq!(read_target, target);

        // Remove symlink
        rt.remove_symlink(&link).unwrap();
        assert!(!rt.exists(&link));
        assert!(rt.exists(&target));
    }

    #[test]
    fn test_real_runtime_file_symlink() {
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let target_file = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");

        // Create target file
        rt.write(&target_file, b"hello").unwrap();

        // Symlink
        rt.symlink(&target_file, &link).unwrap();
        assert!(rt.exists(&link));
        assert!(rt.is_symlink(&link));

        // Read link
        let read_target = rt.read_link(&link).unwrap();
        assert_eq!(read_target, target_file);

        // Verify that we can read the file through the symlink
        let content = rt.read_to_string(&link).unwrap();
        assert_eq!(content, "hello");

        // Remove symlink
        rt.remove_symlink(&link).unwrap();
        assert!(!rt.exists(&link));
        assert!(rt.exists(&target_file));
    }

    #[test]
    fn test_real_runtime_env_and_dirs() {
        let rt = RealRuntime;
        // Test env_var with a likely existing var
        if let Ok(path) = std::env::var("PATH") {
            assert_eq!(rt.env_var("PATH").unwrap(), path);
        }

        assert!(rt.home_dir().is_some());
    }

    #[test]
    fn test_real_runtime_errors() {
        let rt = RealRuntime;
        let dir = tempdir().unwrap();
        let non_existent = dir.path().join("non_existent");

        assert!(rt.read_to_string(&non_existent).is_err());
        assert!(rt.open(&non_existent).is_err());
        assert!(rt.rename(&non_existent, &dir.path().join("new")).is_err());
        assert!(rt.remove_file(&non_existent).is_err());
        assert!(rt.remove_dir(&non_existent).is_err());
    }
}
