mod tar_gz;
mod zip;

use crate::cleanup::SharedCleanupContext;
use crate::runtime::Runtime;
use anyhow::{Result, anyhow};
use std::path::Path;

pub use tar_gz::TarGzExtractor;
pub use zip::ZipExtractor;

/// Trait for format-specific archive extractors
#[cfg_attr(test, mockall::automock)]
pub trait ArchiveExtractor: Send + Sync {
    /// Check if this extractor can handle the given archive format
    fn can_handle(&self, archive_path: &Path) -> bool;

    /// Extract the archive to the specified directory
    fn extract<R: Runtime + 'static>(
        &self,
        runtime: &R,
        archive_path: &Path,
        extract_to: &Path,
    ) -> Result<()>;

    /// Extract the archive with cleanup context for interruption handling
    fn extract_with_cleanup<R: Runtime + 'static>(
        &self,
        runtime: &R,
        archive_path: &Path,
        extract_to: &Path,
        cleanup_ctx: SharedCleanupContext,
    ) -> Result<()>;
}

/// Dispatcher that selects the appropriate extractor based on archive format.
/// Holds all available extractors and dispatches to the correct one.
pub struct ArchiveExtractorImpl {
    tar_gz: TarGzExtractor,
    zip: ZipExtractor,
}

impl Default for ArchiveExtractorImpl {
    fn default() -> Self {
        Self::new()
    }
}

impl ArchiveExtractorImpl {
    pub fn new() -> Self {
        Self {
            tar_gz: TarGzExtractor,
            zip: ZipExtractor,
        }
    }
}

impl ArchiveExtractor for ArchiveExtractorImpl {
    fn can_handle(&self, archive_path: &Path) -> bool {
        self.tar_gz.can_handle(archive_path) || self.zip.can_handle(archive_path)
    }

    #[tracing::instrument(skip(self, runtime, archive_path, extract_to))]
    fn extract<R: Runtime + 'static>(
        &self,
        runtime: &R,
        archive_path: &Path,
        extract_to: &Path,
    ) -> Result<()> {
        if self.tar_gz.can_handle(archive_path) {
            return self.tar_gz.extract(runtime, archive_path, extract_to);
        }
        if self.zip.can_handle(archive_path) {
            return self.zip.extract(runtime, archive_path, extract_to);
        }
        Err(anyhow!(
            "Unsupported archive format: {}",
            archive_path.display()
        ))
    }

    #[tracing::instrument(skip(self, runtime, archive_path, extract_to, cleanup_ctx))]
    fn extract_with_cleanup<R: Runtime + 'static>(
        &self,
        runtime: &R,
        archive_path: &Path,
        extract_to: &Path,
        cleanup_ctx: SharedCleanupContext,
    ) -> Result<()> {
        if self.tar_gz.can_handle(archive_path) {
            return self.tar_gz.extract_with_cleanup(
                runtime,
                archive_path,
                extract_to,
                cleanup_ctx,
            );
        }
        if self.zip.can_handle(archive_path) {
            return self
                .zip
                .extract_with_cleanup(runtime, archive_path, extract_to, cleanup_ctx);
        }
        Err(anyhow!(
            "Unsupported archive format: {}",
            archive_path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RealRuntime;
    use anyhow::Result;
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
    fn test_extractor_impl_can_handle_tar_gz() {
        let extractor = ArchiveExtractorImpl::new();
        assert!(extractor.can_handle(Path::new("file.tar.gz")));
        assert!(extractor.can_handle(Path::new("file.tgz")));
        assert!(extractor.can_handle(Path::new("file.zip")));
        assert!(!extractor.can_handle(Path::new("file.unknown")));
    }

    #[test]
    fn test_extractor_impl_dispatches_to_tar_gz() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.tar.gz");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        create_test_archive(
            &archive_path,
            HashMap::from([("test_dir/file1.txt", "test content")]),
        )?;

        let extractor = ArchiveExtractorImpl::new();
        extractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        let extracted_file = extract_path.join("file1.txt");
        assert!(extracted_file.exists());
        assert_eq!(fs::read_to_string(extracted_file)?, "test content");

        Ok(())
    }

    #[test]
    fn test_extractor_impl_unsupported_format() {
        let extractor = ArchiveExtractorImpl::new();
        let result = extractor.extract(
            &RealRuntime,
            Path::new("/tmp/file.unknown"),
            Path::new("/tmp/out"),
        );
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unsupported archive format")
        );
    }

    fn create_test_zip_archive(path: &Path, files: HashMap<&str, &str>) -> Result<()> {
        use ::zip::CompressionMethod;
        use ::zip::ZipWriter;
        use ::zip::write::FileOptions;
        use std::io::Write;

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
    fn test_extractor_impl_dispatches_to_zip() -> Result<()> {
        let dir = tempdir()?;
        let archive_path = dir.path().join("test.zip");
        let extract_path = dir.path().join("extracted");
        fs::create_dir(&extract_path)?;

        create_test_zip_archive(
            &archive_path,
            HashMap::from([("test_dir/file1.txt", "test content from zip")]),
        )?;

        let extractor = ArchiveExtractorImpl::new();
        extractor.extract(&RealRuntime, &archive_path, &extract_path)?;

        let extracted_file = extract_path.join("file1.txt");
        assert!(extracted_file.exists());
        assert_eq!(fs::read_to_string(extracted_file)?, "test content from zip");

        Ok(())
    }
}
