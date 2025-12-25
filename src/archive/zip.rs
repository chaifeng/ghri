use crate::cleanup::SharedCleanupContext;
use crate::runtime::Runtime;
use anyhow::{Context, Result, anyhow};
use log::{debug, info};
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;

use super::ArchiveExtractor;

/// Extractor for .zip archives
pub struct ZipExtractor;

impl ArchiveExtractor for ZipExtractor {
    fn can_handle(&self, archive_path: &Path) -> bool {
        let name = archive_path.to_string_lossy().to_lowercase();
        name.ends_with(".zip")
    }

    fn extract<R: Runtime + 'static>(
        &self,
        runtime: &R,
        archive_path: &Path,
        extract_to: &Path,
    ) -> Result<()> {
        self.extract_impl(runtime, archive_path, extract_to, None)
    }

    fn extract_with_cleanup<R: Runtime + 'static>(
        &self,
        runtime: &R,
        archive_path: &Path,
        extract_to: &Path,
        cleanup_ctx: SharedCleanupContext,
    ) -> Result<()> {
        self.extract_impl(runtime, archive_path, extract_to, Some(cleanup_ctx))
    }
}

impl ZipExtractor {
    fn extract_impl<R: Runtime + 'static>(
        &self,
        runtime: &R,
        archive_path: &Path,
        extract_to: &Path,
        cleanup_ctx: Option<SharedCleanupContext>,
    ) -> Result<()> {
        debug!("Extracting zip archive to {:?}...", extract_to);
        let file = runtime
            .open(archive_path)
            .with_context(|| format!("Failed to open archive at {:?}", archive_path))?;

        // zip crate requires Read + Seek, but Runtime::open returns Box<dyn Read + Send>
        // We need to read the entire file into memory for seeking capability
        let mut buffer = Vec::new();
        let mut reader = file;
        reader
            .read_to_end(&mut buffer)
            .with_context(|| format!("Failed to read archive {:?}", archive_path))?;
        let cursor = std::io::Cursor::new(buffer);

        let mut archive = ZipArchive::new(cursor).with_context(|| "Failed to parse ZIP archive")?;

        // The archive might have a single top-level directory. We want to extract its contents.
        // We'll extract to a temporary location first to figure out the root dir name.
        let temp_extract_dir = extract_to.with_file_name(format!(
            "{}_temp_extract",
            extract_to.file_name().unwrap().to_string_lossy()
        ));
        if runtime.exists(&temp_extract_dir) {
            runtime.remove_dir_all(&temp_extract_dir)?;
        }
        runtime.create_dir_all(&temp_extract_dir)?;

        // Register temp_extract_dir for cleanup on interruption
        if let Some(ref ctx) = cleanup_ctx {
            let mut guard = ctx.lock().unwrap();
            guard.add(temp_extract_dir.clone());
        }

        debug!("Unpacking to temp dir: {:?}", temp_extract_dir);

        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .with_context(|| format!("Failed to read ZIP entry {}", i))?;

            let entry_path = match entry.enclosed_name() {
                Some(path) => path.to_path_buf(),
                None => {
                    debug!("Skipping entry with invalid path");
                    continue;
                }
            };

            let full_path = temp_extract_dir.join(&entry_path);

            if entry.is_dir() {
                runtime.create_dir_all(&full_path)?;
            } else {
                if let Some(parent) = full_path.parent() {
                    runtime.create_dir_all(parent)?;
                }
                let mut dest_file = runtime.create_file(&full_path)?;
                std::io::copy(&mut entry, &mut dest_file)
                    .with_context(|| format!("Failed to extract file {:?}", full_path))?;

                // Set file permissions from archive metadata (Unix only)
                #[cfg(unix)]
                if let Some(mode) = entry.unix_mode()
                    && let Err(e) = runtime.set_permissions(&full_path, mode)
                {
                    debug!("Failed to set permissions on {:?}: {}", full_path, e);
                }
            }
        }

        // Find the single directory inside the temp extraction dir
        let entries = runtime
            .read_dir(&temp_extract_dir)
            .context("Failed to read temp extraction directory")?;

        if let Some(source_dir) = entries.first() {
            debug!("Found entry in temp dir: {:?}", source_dir);
            let source_dir = if runtime.is_dir(source_dir) && entries.len() == 1 {
                source_dir.clone()
            } else {
                temp_extract_dir.clone()
            };

            // Move contents from temp/{{repo-tag-sha}}/* to {{version}}/*
            debug!("Moving contents from {:?} to {:?}", source_dir, extract_to);
            for item in runtime.read_dir(&source_dir)? {
                let dest_path = extract_to.join(item.file_name().unwrap());
                debug!("Installing {:?}", dest_path);
                runtime.rename(&item, &dest_path)?;
            }
        } else {
            return Err(anyhow!("Archive appears to be empty."));
        }

        // Clean up the temporary extraction directory
        runtime.remove_dir_all(&temp_extract_dir)?;

        // Remove temp_extract_dir from cleanup list
        if let Some(ref ctx) = cleanup_ctx {
            let mut guard = ctx.lock().unwrap();
            guard.remove(&temp_extract_dir);
        }

        info!("Extraction complete.");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RealRuntime;
    use std::collections::HashMap;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::tempdir;
    use zip::CompressionMethod;
    use zip::ZipWriter;
    use zip::write::FileOptions;

    fn create_test_archive(path: &Path, files: HashMap<&str, &str>) -> Result<()> {
        let file = File::create(path)?;
        let mut zip = ZipWriter::new(file);
        let options: FileOptions<()> =
            FileOptions::default().compression_method(CompressionMethod::Deflated);

        for (name, content) in files.iter() {
            zip.start_file(*name, options)?;
            zip.write_all(content.as_bytes())?;
        }

        zip.finish()?;
        Ok(())
    }

    #[test]
    fn test_can_handle_zip() {
        let extractor = ZipExtractor;
        assert!(extractor.can_handle(Path::new("file.zip")));
        assert!(extractor.can_handle(Path::new("FILE.ZIP")));
        assert!(!extractor.can_handle(Path::new("file.tar.gz")));
        assert!(!extractor.can_handle(Path::new("file.tgz")));
    }

    #[test]
    fn test_extract_archive_with_only_one_toplevel_dir() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.zip");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        create_test_archive(
            &archive_path,
            HashMap::from([("test_dir/file1.txt", "test")]),
        )?;

        ZipExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        let extracted_file = extract_path.join("file1.txt");
        assert!(extracted_file.exists());
        assert_eq!(fs::read_to_string(extracted_file)?, "test");

        Ok(())
    }

    #[test]
    fn test_extract_archive_with_multiple_toplevel_dirs() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.zip");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        create_test_archive(
            &archive_path,
            HashMap::from([("foo/file1.txt", "foo1"), ("bar/file2.txt", "bar2")]),
        )?;

        ZipExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        let extracted_file = extract_path.join("foo/file1.txt");
        assert!(extracted_file.exists());
        assert_eq!(fs::read_to_string(extracted_file)?, "foo1");

        let extracted_file = extract_path.join("bar/file2.txt");
        assert!(extracted_file.exists());
        assert_eq!(fs::read_to_string(extracted_file)?, "bar2");

        Ok(())
    }

    #[test]
    fn test_extract_archive_without_toplevel_dir() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.zip");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        create_test_archive(&archive_path, HashMap::from([("file1.txt", "test")]))?;

        ZipExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        let extracted_file = extract_path.join("file1.txt");
        assert!(extracted_file.exists());
        assert_eq!(fs::read_to_string(extracted_file)?, "test");

        Ok(())
    }

    #[test]
    fn test_extract_empty_archive() {
        let dir = tempdir().unwrap();
        let archive_path = dir.path().join("test.zip");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path).unwrap();

        create_test_archive(&archive_path, HashMap::new()).unwrap();

        let result = ZipExtractor.extract(&RealRuntime, &archive_path, &extract_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_corrupted_archive() {
        let dir = tempdir().unwrap();
        let archive_path = dir.path().join("test.zip");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path).unwrap();

        fs::write(&archive_path, "corrupted data").unwrap();

        let result = ZipExtractor.extract(&RealRuntime, &archive_path, &extract_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_temp_dir_already_exists() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.zip");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        let temp_extract_dir = extract_path.with_file_name("extracted_temp_extract");
        fs::create_dir(&temp_extract_dir)?;

        create_test_archive(
            &archive_path,
            HashMap::from([("test_dir/file1.txt", "test")]),
        )?;

        ZipExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        let extracted_file = extract_path.join("file1.txt");
        assert!(extracted_file.exists());
        assert_eq!(fs::read_to_string(extracted_file)?, "test");

        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_extract_archive_preserves_file_permissions() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir()?;
        let archive_path = dir.path().join("test.zip");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        // Create archive with executable file (mode 0o755)
        {
            let file = File::create(&archive_path)?;
            let mut zip = ZipWriter::new(file);

            // Executable script
            let options: FileOptions<()> = FileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .unix_permissions(0o755);
            zip.start_file("test_dir/script.sh", options)?;
            zip.write_all(b"#!/bin/bash\necho hello")?;

            // Regular file
            let options: FileOptions<()> = FileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .unix_permissions(0o644);
            zip.start_file("test_dir/config.txt", options)?;
            zip.write_all(b"some config")?;

            zip.finish()?;
        }

        ZipExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        // Verify executable file has execute permission
        let script_path = extract_path.join("script.sh");
        assert!(script_path.exists());
        let script_mode = fs::metadata(&script_path)?.permissions().mode();
        assert!(
            script_mode & 0o111 != 0,
            "Expected script.sh to be executable, but mode was {:o}",
            script_mode
        );

        // Verify regular file does NOT have execute permission
        let config_path = extract_path.join("config.txt");
        assert!(config_path.exists());
        let config_mode = fs::metadata(&config_path)?.permissions().mode();
        assert!(
            config_mode & 0o111 == 0,
            "Expected config.txt to NOT be executable, but mode was {:o}",
            config_mode
        );

        Ok(())
    }

    #[test]
    fn test_extract_with_cleanup_registers_temp_dir() -> Result<()> {
        use crate::cleanup;

        let dir = tempdir()?;
        let archive_path = dir.path().join("test.zip");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        create_test_archive(
            &archive_path,
            HashMap::from([("test_dir/file1.txt", "test")]),
        )?;

        let cleanup_ctx = cleanup::new_shared();
        ZipExtractor.extract_with_cleanup(
            &RealRuntime,
            &archive_path,
            &extract_path,
            cleanup_ctx.clone(),
        )?;

        // After successful extraction, cleanup context should be empty
        let ctx = cleanup_ctx.lock().unwrap();
        assert!(
            ctx.paths.is_empty(),
            "Cleanup context should be empty after successful extraction"
        );

        let extracted_file = extract_path.join("file1.txt");
        assert!(extracted_file.exists());

        Ok(())
    }

    #[test]
    fn test_extract_archive_with_directory_entries() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.zip");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        // Create archive with explicit directory entries
        {
            let file = File::create(&archive_path)?;
            let mut zip = ZipWriter::new(file);
            let options: FileOptions<()> =
                FileOptions::default().compression_method(CompressionMethod::Stored);

            // Add a directory entry
            zip.add_directory("test_dir/subdir/", options)?;

            // Add a file inside the directory
            let file_options: FileOptions<()> =
                FileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file("test_dir/subdir/file.txt", file_options)?;
            zip.write_all(b"nested file")?;

            zip.finish()?;
        }

        ZipExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        // Verify the directory was created
        let subdir_path = extract_path.join("subdir");
        assert!(subdir_path.is_dir());

        // Verify the file inside was extracted
        let nested_file = extract_path.join("subdir/file.txt");
        assert!(nested_file.exists());
        assert_eq!(fs::read_to_string(nested_file)?, "nested file");

        Ok(())
    }

    #[test]
    fn test_extract_nonexistent_archive() {
        let dir = tempdir().unwrap();
        let archive_path = dir.path().join("nonexistent.zip");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path).unwrap();

        let result = ZipExtractor.extract(&RealRuntime, &archive_path, &extract_path);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to open archive")
        );
    }
}
