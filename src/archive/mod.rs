use crate::runtime::Runtime;
use anyhow::{Context, Result, anyhow};
use flate2::read::GzDecoder;
use log::{debug, info};
use std::path::Path;
use tar::Archive;

pub trait Extractor {
    fn extract<R: Runtime>(
        &self,
        runtime: &R,
        archive_path: &Path,
        extract_to: &Path,
    ) -> Result<()>;
}

pub struct ArchiveExtractor;

impl Extractor for ArchiveExtractor {
    fn extract<R: Runtime>(
        &self,
        runtime: &R,
        archive_path: &Path,
        extract_to: &Path,
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

        debug!("Unpacking to temp dir: {:?}", temp_extract_dir);

        // Use entries() instead of unpack() to use runtime abstraction for all file operations
        for entry in archive
            .entries()
            .context("Failed to read archive entries")?
        {
            let mut entry = entry?;
            let entry_path = entry.path()?.to_path_buf();
            let full_path = temp_extract_dir.join(entry_path);

            if entry.header().entry_type().is_dir() {
                runtime.create_dir_all(&full_path)?;
            } else {
                if let Some(parent) = full_path.parent() {
                    runtime.create_dir_all(parent)?;
                }
                let mut dest_file = runtime.create_file(&full_path)?;
                std::io::copy(&mut entry, &mut dest_file)
                    .with_context(|| format!("Failed to extract file {:?}", full_path))?;
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
}
