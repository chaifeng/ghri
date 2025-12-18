use anyhow::{Context, Result, anyhow};
use flate2::read::GzDecoder;
use log::{debug, info};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use tar::Archive;

/// Extracts the tar.gz archive to the specified directory.
pub fn extract_archive(archive_path: &Path, extract_to: &Path) -> Result<()> {
    debug!("Extracting archive to {:?}...", extract_to);
    let file = File::open(archive_path)
        .with_context(|| format!("Failed to open archive at {:?}", archive_path))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    // The archive might have a single top-level directory. We want to extract its contents.
    // We'll extract to a temporary location first to figure out the root dir name.
    let temp_extract_dir = extract_to.with_file_name(format!(
        "{}_temp_extract",
        extract_to.file_name().unwrap().to_string_lossy()
    ));
    if temp_extract_dir.exists() {
        fs::remove_dir_all(&temp_extract_dir)?;
    }
    fs::create_dir_all(&temp_extract_dir)?;

    debug!("Unpacking to temp dir: {:?}", temp_extract_dir);
    archive
        .unpack(&temp_extract_dir)
        .with_context(|| format!("Failed to unpack archive to {:?}", temp_extract_dir))?;

    // Find the single directory inside the temp extraction dir
    let mut entries =
        fs::read_dir(&temp_extract_dir).context("Failed to read temp extraction directory")?;

    if let Some(Ok(entry)) = entries.next() {
        let source_dir = entry.path();
        debug!("Found entry in temp dir: {:?}", source_dir);
        let source_dir = if source_dir.is_dir() && entries.next().is_none() {
            source_dir
        } else {
            PathBuf::from(&temp_extract_dir)
        };

        // Move contents from temp/{{repo-tag-sha}}/* to {{version}}/*
        debug!("Entry is a directory, moving contents");
        for item in fs::read_dir(&source_dir)? {
            let item = item?;
            let dest_path = extract_to.join(item.file_name());
            debug!("Installing {:?}", dest_path);
            fs::rename(item.path(), &dest_path)?;
        }
    } else {
        return Err(anyhow!("Archive appears to be empty."));
    }

    // Clean up the temporary extraction directory
    fs::remove_dir_all(&temp_extract_dir)?;

    info!("Extraction complete.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::collections::HashMap;
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

        extract_archive(&archive_path, &extract_path)?;

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

        extract_archive(&archive_path, &extract_path)?;

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

        extract_archive(&archive_path, &extract_path)?;

        let extracted_file = extract_path.join("file1.txt");
        assert!(extracted_file.exists());
        assert_eq!(fs::read_to_string(extracted_file)?, "test");

        Ok(())
    }
}
