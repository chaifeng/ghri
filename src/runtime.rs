use anyhow::{Context, Result};
use async_trait::async_trait;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[async_trait]
pub trait Runtime: Send + Sync {
    // Environment
    fn env_var(&self, key: &str) -> Result<String, env::VarError>;

    // File System
    fn write(&self, path: &Path, contents: impl AsRef<[u8]> + Send) -> Result<()>;
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
    fn env_var(&self, key: &str) -> Result<String, env::VarError> {
        env::var(key)
    }

    fn write(&self, path: &Path, contents: impl AsRef<[u8]> + Send) -> Result<()> {
        fs::write(path, contents).context("Failed to write to file")?;
        Ok(())
    }

    fn read_to_string(&self, path: &Path) -> Result<String> {
        fs::read_to_string(path).context("Failed to read file to string")
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        fs::rename(from, to).context("Failed to rename file")?;
        Ok(())
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path).context("Failed to create directory")?;
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        fs::remove_file(path).context("Failed to remove file")?;
        Ok(())
    }

    fn remove_dir(&self, path: &Path) -> Result<()> {
        fs::remove_dir(path).context("Failed to remove directory")?;
        Ok(())
    }

    fn remove_symlink(&self, path: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            fs::remove_file(path).context("Failed to remove symlink")?;
        }
        #[cfg(windows)]
        {
            let metadata = fs::symlink_metadata(path).context("Failed to get symlink metadata")?;
            if metadata.file_type().is_dir() {
                fs::remove_dir(path).context("Failed to remove directory symlink")?;
            } else {
                fs::remove_file(path).context("Failed to remove file symlink")?;
            }
        }
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        fs::read_dir(path)?.map(|entry| Ok(entry?.path())).collect()
    }

    fn symlink(&self, original: &Path, link: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink as unix_symlink;
            unix_symlink(original, link).context("Failed to create symlink")?;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_dir;
            symlink_dir(original, link).context("Failed to create symlink")?;
        }
        Ok(())
    }

    fn read_link(&self, path: &Path) -> Result<PathBuf> {
        fs::read_link(path).context("Failed to read symlink")
    }

    fn is_symlink(&self, path: &Path) -> bool {
        fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }

    fn create_file(&self, path: &Path) -> Result<Box<dyn std::io::Write + Send>> {
        let file = std::fs::File::create(path).context("Failed to create file")?;
        Ok(Box::new(file))
    }

    fn open(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>> {
        let file = std::fs::File::open(path).context("Failed to open file")?;
        Ok(Box::new(file))
    }

    fn remove_dir_all(&self, path: &Path) -> Result<()> {
        fs::remove_dir_all(path).context("Failed to remove directory and its contents")?;
        Ok(())
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn home_dir(&self) -> Option<PathBuf> {
        dirs::home_dir()
    }

    fn config_dir(&self) -> Option<PathBuf> {
        dirs::config_dir()
    }
}

#[cfg(test)]
pub struct MockRuntime {
    pub env_vars: std::collections::HashMap<String, String>,
    pub files: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<PathBuf, Vec<u8>>>>,
}

#[cfg(test)]
impl MockRuntime {
    pub fn new() -> Self {
        Self {
            env_vars: std::collections::HashMap::new(),
            files: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }
}

#[cfg(test)]
struct MockFile {
    path: PathBuf,
    files: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<PathBuf, Vec<u8>>>>,
}

#[cfg(test)]
impl std::io::Write for MockFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut files = self.files.lock().unwrap();
        files
            .entry(self.path.clone())
            .or_default()
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[async_trait]
impl Runtime for MockRuntime {
    fn env_var(&self, key: &str) -> Result<String, std::env::VarError> {
        self.env_vars
            .get(key)
            .cloned()
            .ok_or(std::env::VarError::NotPresent)
    }

    fn write(&self, path: &Path, contents: impl AsRef<[u8]> + Send) -> Result<()> {
        let mut files = self.files.lock().unwrap();
        files.insert(path.to_path_buf(), contents.as_ref().to_vec());
        Ok(())
    }

    fn read_to_string(&self, path: &Path) -> Result<String> {
        let files = self.files.lock().unwrap();
        files
            .get(path)
            .map(|content| String::from_utf8_lossy(content).into_owned())
            .ok_or_else(|| anyhow::anyhow!("File not found in MockRuntime: {:?}", path))
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        let mut files = self.files.lock().unwrap();
        if let Some(content) = files.remove(from) {
            files.insert(to.to_path_buf(), content);
            Ok(())
        } else {
            Err(anyhow::anyhow!("File not found in MockRuntime: {:?}", from))
        }
    }

    fn create_dir_all(&self, _path: &Path) -> Result<()> {
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        let mut files = self.files.lock().unwrap();
        files.remove(path);
        Ok(())
    }

    fn remove_dir(&self, path: &Path) -> Result<()> {
        // In MockRuntime, directories are not explicitly stored,
        // but we can simulate removal by removing all files with this prefix.
        let mut files = self.files.lock().unwrap();
        files.retain(|p, _| !p.starts_with(path));
        Ok(())
    }

    fn remove_symlink(&self, path: &Path) -> Result<()> {
        // In MockRuntime, we treat symlinks as files.
        self.remove_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        let files = self.files.lock().unwrap();
        files.contains_key(path)
    }

    fn read_dir(&self, _path: &Path) -> Result<Vec<PathBuf>> {
        Ok(vec![])
    }

    fn symlink(&self, _original: &Path, _link: &Path) -> Result<()> {
        Ok(())
    }

    fn read_link(&self, _path: &Path) -> Result<PathBuf> {
        Ok(PathBuf::new())
    }

    fn is_symlink(&self, _path: &Path) -> bool {
        false
    }

    fn create_file(&self, path: &Path) -> Result<Box<dyn std::io::Write + Send>> {
        Ok(Box::new(MockFile {
            path: path.to_path_buf(),
            files: self.files.clone(),
        }))
    }

    fn open(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>> {
        let files = self.files.lock().unwrap();
        if let Some(content) = files.get(path) {
            Ok(Box::new(std::io::Cursor::new(content.clone())))
        } else {
            Err(anyhow::anyhow!("File not found in MockRuntime: {:?}", path))
        }
    }

    fn remove_dir_all(&self, path: &Path) -> Result<()> {
        let mut files = self.files.lock().unwrap();
        files.retain(|p, _| !p.starts_with(path));
        Ok(())
    }

    fn is_dir(&self, path: &Path) -> bool {
        let files = self.files.lock().unwrap();
        // A path is a directory if there are any files starting with that path/
        files.keys().any(|p| p.starts_with(path) && p != path)
    }

    fn home_dir(&self) -> Option<PathBuf> {
        Some(PathBuf::from("/home/user"))
    }

    fn config_dir(&self) -> Option<PathBuf> {
        Some(PathBuf::from("/home/user/.config"))
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
        rt.write(&file_path, "hello").unwrap();
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
        rt.write(&sub_dir_full.join("file.txt"), "data").unwrap();
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
    fn test_real_runtime_env_and_dirs() {
        let rt = RealRuntime;
        // Test env_var with a likely existing var
        if let Ok(path) = std::env::var("PATH") {
            assert_eq!(rt.env_var("PATH").unwrap(), path);
        }

        assert!(rt.home_dir().is_some());
    }

    #[test]
    fn test_mock_runtime_file_ops() {
        let rt = MockRuntime::new();
        let path = Path::new("/test/file.txt");

        // Write/Exists
        rt.write(path, "mock data").unwrap();
        assert!(rt.exists(path));

        // Read
        assert_eq!(rt.read_to_string(path).unwrap(), "mock data");

        // Rename
        let new_path = Path::new("/test/new_file.txt");
        rt.rename(path, new_path).unwrap();
        assert!(!rt.exists(path));
        assert!(rt.exists(new_path));
        assert_eq!(rt.read_to_string(new_path).unwrap(), "mock data");

        // Open
        let mut reader = rt.open(new_path).unwrap();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, b"mock data");

        // Create with stream
        let path2 = Path::new("/test/file2.txt");
        {
            let mut writer = rt.create_file(path2).unwrap();
            writer.write_all(b"streamed data").unwrap();
        }
        assert_eq!(rt.read_to_string(path2).unwrap(), "streamed data");

        // Remove
        rt.remove_file(new_path).unwrap();
        assert!(!rt.exists(new_path));
    }

    #[test]
    fn test_mock_runtime_dir_simulation() {
        let rt = MockRuntime::new();
        let file1 = Path::new("/a/b/file1.txt");
        let file2 = Path::new("/a/c/file2.txt");

        rt.write(file1, "1").unwrap();
        rt.write(file2, "2").unwrap();

        // is_dir simulation
        assert!(rt.is_dir(Path::new("/a")));
        assert!(rt.is_dir(Path::new("/a/b")));
        assert!(!rt.is_dir(file1));

        // remove_dir (removes all files with prefix)
        rt.remove_dir(Path::new("/a/b")).unwrap();
        assert!(!rt.exists(file1));
        assert!(rt.exists(file2));

        // remove_dir_all
        rt.remove_dir_all(Path::new("/a")).unwrap();
        assert!(!rt.exists(file2));
    }

    #[test]
    fn test_mock_runtime_env_and_dirs() {
        let mut rt = MockRuntime::new();
        rt.env_vars.insert("KEY".to_string(), "VALUE".to_string());

        assert_eq!(rt.env_var("KEY").unwrap(), "VALUE");
        assert!(rt.env_var("NONEXISTENT").is_err());

        assert_eq!(rt.home_dir().unwrap(), PathBuf::from("/home/user"));
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

    #[test]
    fn test_mock_runtime_errors() {
        let rt = MockRuntime::new();
        let path = Path::new("/none");

        assert!(rt.read_to_string(path).is_err());
        assert!(rt.open(path).is_err());
        assert!(rt.rename(path, Path::new("/new")).is_err());
    }
}
