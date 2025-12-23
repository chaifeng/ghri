//! Runtime abstraction for system operations.
//!
//! This module provides a trait-based abstraction over system operations,
//! enabling dependency injection and testability.
//!
//! # Structure
//!
//! - `path` - Path utility functions (normalize, is_path_under, relative_symlink_path)
//! - `env` - Environment variables and system information
//! - `fs` - File system operations (read, write, directory)
//! - `symlink` - Symlink operations (create, read, resolve, remove)
//! - `user` - User interaction (confirmation prompts)

mod env;
mod fs;
pub mod path;
mod symlink;
mod user;

use anyhow::Result;
use async_trait::async_trait;
use std::env as std_env;
use std::path::{Path, PathBuf};

pub use path::{is_path_under, relative_symlink_path};

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait Runtime: Send + Sync {
    // Environment
    fn env_var(&self, key: &str) -> Result<String, std_env::VarError>;

    // File System
    fn write(&self, path: &Path, contents: &[u8]) -> Result<()>;
    fn read_to_string(&self, path: &Path) -> Result<String>;
    fn rename(&self, from: &Path, to: &Path) -> Result<()>;
    fn copy(&self, from: &Path, to: &Path) -> Result<u64>;
    fn create_dir_all(&self, path: &Path) -> Result<()>;
    fn remove_file(&self, path: &Path) -> Result<()>;
    fn remove_dir(&self, path: &Path) -> Result<()>;
    fn remove_symlink(&self, path: &Path) -> Result<()>;
    fn exists(&self, path: &Path) -> bool;
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>>;
    fn symlink(&self, original: &Path, link: &Path) -> Result<()>;
    fn read_link(&self, path: &Path) -> Result<PathBuf>;

    /// Resolve a symlink to an absolute path (without recursively resolving symlinks).
    /// If the link target is relative, it is resolved relative to the link's parent directory.
    /// Unlike canonicalize, this does not follow nested symlinks.
    fn resolve_link(&self, path: &Path) -> Result<PathBuf>;

    /// Canonicalize a path by resolving all symlinks and returning the canonical absolute path.
    /// This recursively resolves all symlinks in the path.
    fn canonicalize(&self, path: &Path) -> Result<PathBuf>;

    fn is_symlink(&self, path: &Path) -> bool;
    fn create_file(&self, path: &Path) -> Result<Box<dyn std::io::Write + Send>>;
    fn open(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>>;
    fn remove_dir_all(&self, path: &Path) -> Result<()>;
    fn is_dir(&self, path: &Path) -> bool;

    /// Set file permissions (mode) on Unix systems. No-op on Windows.
    fn set_permissions(&self, path: &Path, mode: u32) -> Result<()>;

    /// Remove a symlink if its target is under the given prefix directory.
    /// The prefix is checked by directory components, not string prefix.
    /// Returns Ok(true) if removed, Ok(false) if skipped, Err if operation failed.
    fn remove_symlink_if_target_under(
        &self,
        link_path: &Path,
        target_prefix: &Path,
        description: &str,
    ) -> Result<bool>;

    // Directories
    fn home_dir(&self) -> Option<PathBuf>;
    fn config_dir(&self) -> Option<PathBuf>;

    // Privilege
    fn is_privileged(&self) -> bool;

    // User interaction
    /// Prompt user for confirmation. Returns true if user confirms (y/yes), false otherwise.
    fn confirm(&self, prompt: &str) -> Result<bool>;
}

pub struct RealRuntime;

#[async_trait]
impl Runtime for RealRuntime {
    fn env_var(&self, key: &str) -> Result<String, std_env::VarError> {
        self.env_var_impl(key)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> Result<()> {
        self.write_impl(path, contents)
    }

    fn read_to_string(&self, path: &Path) -> Result<String> {
        self.read_to_string_impl(path)
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        self.rename_impl(from, to)
    }

    fn copy(&self, from: &Path, to: &Path) -> Result<u64> {
        self.copy_impl(from, to)
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        self.create_dir_all_impl(path)
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        self.remove_file_impl(path)
    }

    fn remove_dir(&self, path: &Path) -> Result<()> {
        self.remove_dir_impl(path)
    }

    fn remove_symlink(&self, path: &Path) -> Result<()> {
        self.remove_symlink_impl(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.exists_impl(path)
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        self.read_dir_impl(path)
    }

    fn symlink(&self, original: &Path, link: &Path) -> Result<()> {
        self.symlink_impl(original, link)
    }

    fn read_link(&self, path: &Path) -> Result<PathBuf> {
        self.read_link_impl(path)
    }

    fn resolve_link(&self, path: &Path) -> Result<PathBuf> {
        self.resolve_link_impl(path)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        self.canonicalize_impl(path)
    }

    fn is_symlink(&self, path: &Path) -> bool {
        self.is_symlink_impl(path)
    }

    fn create_file(&self, path: &Path) -> Result<Box<dyn std::io::Write + Send>> {
        self.create_file_impl(path)
    }

    fn open(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>> {
        self.open_impl(path)
    }

    fn remove_dir_all(&self, path: &Path) -> Result<()> {
        self.remove_dir_all_impl(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.is_dir_impl(path)
    }

    fn set_permissions(&self, path: &Path, mode: u32) -> Result<()> {
        self.set_permissions_impl(path, mode)
    }

    fn remove_symlink_if_target_under(
        &self,
        link_path: &Path,
        target_prefix: &Path,
        description: &str,
    ) -> Result<bool> {
        self.remove_symlink_if_target_under_impl(link_path, target_prefix, description)
    }

    fn home_dir(&self) -> Option<PathBuf> {
        self.home_dir_impl()
    }

    fn config_dir(&self) -> Option<PathBuf> {
        self.config_dir_impl()
    }

    fn is_privileged(&self) -> bool {
        self.is_privileged_impl()
    }

    fn confirm(&self, prompt: &str) -> Result<bool> {
        self.confirm_impl(prompt)
    }
}
