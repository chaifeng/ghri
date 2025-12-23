use crate::cleanup::SharedCleanupContext;
use crate::runtime::Runtime;
use anyhow::{Context, Result, anyhow};
use flate2::read::GzDecoder;
use log::{debug, info};
use std::path::Path;
use tar::Archive;

#[cfg_attr(test, mockall::automock)]
pub trait Extractor {
    fn extract<R: Runtime + 'static>(
        &self,
        runtime: &R,
        archive_path: &Path,
        extract_to: &Path,
    ) -> Result<()>;

    fn extract_with_cleanup<R: Runtime + 'static>(
        &self,
        runtime: &R,
        archive_path: &Path,
        extract_to: &Path,
        cleanup_ctx: SharedCleanupContext,
    ) -> Result<()>;
}

pub struct ArchiveExtractor;

impl Extractor for ArchiveExtractor {
    #[tracing::instrument(skip(self, runtime, archive_path, extract_to))]
    fn extract<R: Runtime + 'static>(
        &self,
        runtime: &R,
        archive_path: &Path,
        extract_to: &Path,
    ) -> Result<()> {
        self.extract_impl(runtime, archive_path, extract_to, None)
    }

    #[tracing::instrument(skip(self, runtime, archive_path, extract_to, cleanup_ctx))]
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

impl ArchiveExtractor {
    fn extract_impl<R: Runtime + 'static>(
        &self,
        runtime: &R,
        archive_path: &Path,
        extract_to: &Path,
        cleanup_ctx: Option<SharedCleanupContext>,
    ) -> Result<()> {
        debug!("Extracting archive to {:?}...", extract_to);
        let file = runtime
            .open(archive_path)
            .with_context(|| format!("Failed to open archive at {:?}", archive_path))?;
        let decoder = GzDecoder::new(file);
        let mut archive = Archive::new(decoder);

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

        // Use entries() instead of unpack() to use runtime abstraction for all file operations
        for entry in archive
            .entries()
            .context("Failed to read archive entries")?
        {
            let mut entry = entry?;
            let entry_type = entry.header().entry_type();

            // Skip PAX global/extended headers - these are metadata entries, not actual files
            if entry_type == tar::EntryType::XGlobalHeader || entry_type == tar::EntryType::XHeader
            {
                debug!("Skipping PAX header entry");
                continue;
            }

            let entry_path = entry.path()?.to_path_buf();
            let full_path = temp_extract_dir.join(&entry_path);

            if entry_type.is_dir() {
                runtime.create_dir_all(&full_path)?;
            } else if entry_type.is_file() {
                if let Some(parent) = full_path.parent() {
                    runtime.create_dir_all(parent)?;
                }
                let mut dest_file = runtime.create_file(&full_path)?;
                std::io::copy(&mut entry, &mut dest_file)
                    .with_context(|| format!("Failed to extract file {:?}", full_path))?;

                // Set file permissions from archive metadata
                if let Ok(mode) = entry.header().mode()
                    && let Err(e) = runtime.set_permissions(&full_path, mode)
                {
                    debug!("Failed to set permissions on {:?}: {}", full_path, e);
                }
            } else if entry_type.is_symlink()
                && let Some(link_name) = entry.link_name()?
            {
                if let Some(parent) = full_path.parent() {
                    runtime.create_dir_all(parent)?;
                }
                if let Err(e) = runtime.symlink(link_name.as_ref(), &full_path) {
                    debug!(
                        "Failed to create symlink {:?} -> {:?}: {}",
                        full_path, link_name, e
                    );
                }
            }
            // Skip other entry types (hard links, etc.)
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
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::collections::HashMap;
    use std::fs::{self, File};
    use tar::Builder;
    use tempfile::tempdir;

    fn create_test_archive(path: &Path, files: HashMap<&str, &str>) -> Result<()> {
        let file = File::create(path)?;
        let enc = GzEncoder::new(file, Compression::default());
        let mut tar = Builder::new(enc);

        let mut header = tar::Header::new_gnu();
        for (f, content) in files.iter() {
            header.set_path(f)?;
            header.set_size(content.len() as u64);
            header.set_cksum();
            tar.append(&header, content.as_bytes())?;
        }

        tar.finish()?;
        Ok(())
    }

    #[test]
    fn test_extract_archive_with_only_one_toplevel_dir() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        create_test_archive(
            &archive_path,
            HashMap::from([("test_dir/file1.txt", "test")]),
        )?;

        ArchiveExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        let extracted_file = extract_path.join("file1.txt");
        assert!(extracted_file.exists());
        assert_eq!(fs::read_to_string(extracted_file)?, "test");

        Ok(())
    }

    #[test]
    fn test_extract_archive_with_multiple_toplevel_dirs() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        create_test_archive(
            &archive_path,
            HashMap::from([("foo/file1.txt", "foo1"), ("bar/file2.txt", "bar2")]),
        )?;

        ArchiveExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

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
        let archive_path = dir.path().join("test.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        create_test_archive(&archive_path, HashMap::from([("file1.txt", "test")]))?;

        ArchiveExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        let extracted_file = extract_path.join("file1.txt");
        assert!(extracted_file.exists());
        assert_eq!(fs::read_to_string(extracted_file)?, "test");

        Ok(())
    }

    #[test]
    fn test_extract_empty_archive() {
        let dir = tempdir().unwrap();
        let archive_path = dir.path().join("test.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path).unwrap();

        create_test_archive(&archive_path, HashMap::new()).unwrap();

        let result = ArchiveExtractor.extract(&RealRuntime, &archive_path, &extract_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_corrupted_archive() {
        let dir = tempdir().unwrap();
        let archive_path = dir.path().join("test.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path).unwrap();

        fs::write(&archive_path, "corrupted data").unwrap();

        let result = ArchiveExtractor.extract(&RealRuntime, &archive_path, &extract_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_temp_dir_already_exists() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        let temp_extract_dir = extract_path.with_file_name("extracted_temp_extract");
        fs::create_dir(&temp_extract_dir)?;

        create_test_archive(
            &archive_path,
            HashMap::from([("test_dir/file1.txt", "test")]),
        )?;

        ArchiveExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        let extracted_file = extract_path.join("file1.txt");
        assert!(extracted_file.exists());
        assert_eq!(fs::read_to_string(extracted_file)?, "test");

        Ok(())
    }

    /// Creates a test archive with PAX global header (like GitHub's source tarballs)
    fn create_pax_archive(path: &Path, files: HashMap<&str, &str>) -> Result<()> {
        let file = File::create(path)?;
        let enc = GzEncoder::new(file, Compression::default());
        let mut tar = Builder::new(enc);

        // Add a PAX global header entry (this is what GitHub adds to source tarballs)
        let mut pax_header = tar::Header::new_ustar();
        pax_header.set_entry_type(tar::EntryType::XGlobalHeader);
        pax_header.set_path("pax_global_header")?;
        let pax_data = b"52 comment=some git commit hash here\n";
        pax_header.set_size(pax_data.len() as u64);
        pax_header.set_cksum();
        tar.append(&pax_header, &pax_data[..])?;

        // Add actual files
        let mut header = tar::Header::new_gnu();
        for (f, content) in files.iter() {
            header.set_path(f)?;
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append(&header, content.as_bytes())?;
        }

        tar.finish()?;
        Ok(())
    }

    #[test]
    fn test_extract_archive_skips_pax_global_header() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        create_pax_archive(
            &archive_path,
            HashMap::from([("test_dir/file1.txt", "test content")]),
        )?;

        ArchiveExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        // Verify the actual file was extracted
        let extracted_file = extract_path.join("file1.txt");
        assert!(extracted_file.exists());
        assert_eq!(fs::read_to_string(&extracted_file)?, "test content");

        // Verify pax_global_header was NOT extracted
        assert!(!extract_path.join("pax_global_header").exists());

        // Verify no other unexpected files exist
        let entries: Vec<_> = fs::read_dir(&extract_path)?
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "Expected only file1.txt, but found: {:?}",
            entries
        );

        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_extract_archive_preserves_file_permissions() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir()?;
        let archive_path = dir.path().join("test.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        // Create archive with executable file (mode 0o755)
        {
            let file = File::create(&archive_path)?;
            let enc = GzEncoder::new(file, Compression::default());
            let mut tar = Builder::new(enc);

            let mut header = tar::Header::new_gnu();
            header.set_path("test_dir/script.sh")?;
            let content = b"#!/bin/bash\necho hello";
            header.set_size(content.len() as u64);
            header.set_mode(0o755); // Executable
            header.set_cksum();
            tar.append(&header, &content[..])?;

            // Add a regular file (mode 0o644)
            let mut header2 = tar::Header::new_gnu();
            header2.set_path("test_dir/config.txt")?;
            let config_content = b"some config";
            header2.set_size(config_content.len() as u64);
            header2.set_mode(0o644); // Regular file
            header2.set_cksum();
            tar.append(&header2, &config_content[..])?;

            tar.finish()?;
        }

        ArchiveExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

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
        let archive_path = dir.path().join("test.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        create_test_archive(
            &archive_path,
            HashMap::from([("test_dir/file1.txt", "test")]),
        )?;

        let cleanup_ctx = cleanup::new_shared();
        ArchiveExtractor.extract_with_cleanup(
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
    #[cfg(unix)]
    fn test_extract_archive_with_symlinks() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        // Create archive with a symlink
        {
            let file = File::create(&archive_path)?;
            let enc = GzEncoder::new(file, Compression::default());
            let mut tar = Builder::new(enc);

            // Add a regular file
            let mut header = tar::Header::new_gnu();
            header.set_path("test_dir/target.txt")?;
            let content = b"target content";
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append(&header, &content[..])?;

            // Add a symlink pointing to the file
            let mut link_header = tar::Header::new_gnu();
            link_header.set_path("test_dir/link.txt")?;
            link_header.set_entry_type(tar::EntryType::Symlink);
            link_header.set_size(0);
            link_header.set_link_name("target.txt")?;
            link_header.set_cksum();
            tar.append(&link_header, &[] as &[u8])?;

            tar.finish()?;
        }

        ArchiveExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        // Verify the target file was extracted
        let target_path = extract_path.join("target.txt");
        assert!(target_path.exists());

        // Verify the symlink was created
        let link_path = extract_path.join("link.txt");
        assert!(link_path.is_symlink() || link_path.exists());

        Ok(())
    }

    #[test]
    fn test_extract_archive_with_directory_entries() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        // Create archive with explicit directory entries
        {
            let file = File::create(&archive_path)?;
            let enc = GzEncoder::new(file, Compression::default());
            let mut tar = Builder::new(enc);

            // Add a directory entry
            let mut dir_header = tar::Header::new_gnu();
            dir_header.set_path("test_dir/subdir/")?;
            dir_header.set_entry_type(tar::EntryType::Directory);
            dir_header.set_size(0);
            dir_header.set_mode(0o755);
            dir_header.set_cksum();
            tar.append(&dir_header, &[] as &[u8])?;

            // Add a file inside the directory
            let mut file_header = tar::Header::new_gnu();
            file_header.set_path("test_dir/subdir/file.txt")?;
            let content = b"nested file";
            file_header.set_size(content.len() as u64);
            file_header.set_mode(0o644);
            file_header.set_cksum();
            tar.append(&file_header, &content[..])?;

            tar.finish()?;
        }

        ArchiveExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

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
    fn test_extract_archive_with_xheader() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        // Create archive with XHeader (extended header)
        {
            let file = File::create(&archive_path)?;
            let enc = GzEncoder::new(file, Compression::default());
            let mut tar = Builder::new(enc);

            // Add an extended header entry
            let mut xheader = tar::Header::new_ustar();
            xheader.set_entry_type(tar::EntryType::XHeader);
            xheader.set_path("./PaxHeaders.0/file.txt")?;
            let xheader_data = b"30 mtime=1234567890.123456789\n";
            xheader.set_size(xheader_data.len() as u64);
            xheader.set_cksum();
            tar.append(&xheader, &xheader_data[..])?;

            // Add actual file
            let mut header = tar::Header::new_gnu();
            header.set_path("test_dir/file.txt")?;
            let content = b"file content";
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append(&header, &content[..])?;

            tar.finish()?;
        }

        ArchiveExtractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        // Verify the actual file was extracted
        let extracted_file = extract_path.join("file.txt");
        assert!(extracted_file.exists());
        assert_eq!(fs::read_to_string(&extracted_file)?, "file content");

        // Verify XHeader was NOT extracted as a file
        assert!(!extract_path.join("PaxHeaders.0").exists());

        Ok(())
    }

    #[test]
    fn test_extract_nonexistent_archive() {
        let dir = tempdir().unwrap();
        let archive_path = dir.path().join("nonexistent.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path).unwrap();

        let result = ArchiveExtractor.extract(&RealRuntime, &archive_path, &extract_path);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to open archive")
        );
    }
}
