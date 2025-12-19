use crate::{
    archive::Extractor,
    download::download_file,
    github::{GetReleases, GitHubRepo, Release, ReleaseAsset, RepoInfo},
};
use anyhow::{Context, Result, bail};
use log::{debug, info, warn};
use reqwest::Client;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

pub mod config;
use config::Config;

pub async fn install(repo_str: &str, install_root: Option<PathBuf>) -> Result<()> {
    let config = Config::new(install_root)?;
    run(repo_str, config).await
}

pub async fn run<G: GetReleases, E: Extractor>(
    repo_str: &str,
    config: Config<G, E>,
) -> Result<()> {
    let repo = repo_str.parse::<GitHubRepo>()?;
    let installer = Installer::new(config.github, config.client, config.extractor);
    installer.install(&repo, config.install_root).await
}

pub struct Installer<G: GetReleases, E: Extractor> {
    pub github: G,
    pub client: Client,
    pub extractor: E,
}

impl<G: GetReleases, E: Extractor> Installer<G, E> {
    pub fn new(github: G, client: Client, extractor: E) -> Self {
        Self {
            github,
            client,
            extractor,
        }
    }

    pub async fn install(&self, repo: &GitHubRepo, install_root: Option<PathBuf>) -> Result<()> {
        let release = self.github.get_latest_release(repo).await?;
        info!("Found latest version: {}", release.tag_name);

        let target_dir = get_target_dir(&repo, &release, install_root)?;

        ensure_installed(&target_dir, &repo, &release, &self.client, &self.extractor).await?;
        update_current_symlink(&target_dir, &release.tag_name)?;

        // Metadata handling
        if let Err(e) = self
            .save_metadata(&repo, &release.tag_name, &target_dir)
            .await
        {
            warn!("Failed to save package metadata: {}. Continuing.", e);
        }

        self.print_install_success(repo, &release.tag_name, &target_dir);

        Ok(())
    }

    fn print_install_success(&self, repo: &GitHubRepo, tag: &str, target_dir: &Path) {
        println!(
            "installed {}/{} {} {}",
            repo.owner,
            repo.repo,
            tag,
            target_dir.display()
        );
    }

    async fn save_metadata(
        &self,
        repo: &GitHubRepo,
        current_version: &str,
        target_dir: &Path,
    ) -> Result<()> {
        let package_root = target_dir.parent().context("Failed to get package root")?;
        let meta_path = package_root.join("meta.json");

        let repo_info = self.github.get_repo_info(repo).await?;
        let releases = self.github.get_releases(repo).await?;

        let meta = Meta::from(repo.clone(), repo_info, releases, current_version);
        let json = serde_json::to_string_pretty(&meta)?;
        fs::write(&meta_path, json).context("Failed to write meta.json")?;

        Ok(())
    }
}

#[derive(Serialize)]
struct Meta {
    name: String,
    description: Option<String>,
    homepage: Option<String>,
    license: Option<String>,
    updated_at: String,
    current_version: String,
    releases: Vec<MetaRelease>,
}

impl Meta {
    fn from(repo: GitHubRepo, info: RepoInfo, releases: Vec<Release>, current: &str) -> Self {
        Meta {
            name: format!("{}/{}", repo.owner, repo.repo),
            description: info.description,
            homepage: info.homepage,
            license: info.license.map(|l| l.name),
            updated_at: info.updated_at,
            current_version: current.to_string(),
            releases: releases.into_iter().map(MetaRelease::from).collect(),
        }
    }
}

#[derive(Serialize)]
struct MetaRelease {
    version: String,
    title: Option<String>,
    published_at: Option<String>,
    is_prerelease: bool,
    tarball_url: String,
    assets: Vec<MetaAsset>,
}

impl From<Release> for MetaRelease {
    fn from(r: Release) -> Self {
        MetaRelease {
            version: r.tag_name,
            title: r.name,
            published_at: r.published_at,
            is_prerelease: r.prerelease,
            tarball_url: r.tarball_url,
            assets: r.assets.into_iter().map(MetaAsset::from).collect(),
        }
    }
}

#[derive(Serialize)]
struct MetaAsset {
    name: String,
    size: u64,
    download_url: String,
}

impl From<ReleaseAsset> for MetaAsset {
    fn from(a: ReleaseAsset) -> Self {
        MetaAsset {
            name: a.name,
            size: a.size,
            download_url: a.browser_download_url,
        }
    }
}

fn get_target_dir(
    repo: &GitHubRepo,
    release: &Release,
    install_root: Option<PathBuf>,
) -> Result<PathBuf> {
    let root = match install_root {
        Some(path) => path,
        None => default_install_root()?,
    };

    info!("Using install root: {}", root.display());

    Ok(root
        .join(&repo.owner)
        .join(&repo.repo)
        .join(&release.tag_name))
}

fn default_install_root() -> Result<PathBuf> {
    if is_privileged() {
        Ok(system_install_root())
    } else {
        let home_dir = dirs::home_dir().context("Could not find home directory")?;
        Ok(home_dir.join(".ghri"))
    }
}

#[cfg(target_os = "macos")]
fn system_install_root() -> PathBuf {
    PathBuf::from("/opt/ghri")
}

#[cfg(target_os = "windows")]
fn system_install_root() -> PathBuf {
    PathBuf::from(r"C:\ProgramData\ghri")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn system_install_root() -> PathBuf {
    PathBuf::from("/usr/local/ghri")
}

#[cfg(all(unix, not(feature = "test_in_root")))]
fn is_privileged() -> bool {
    nix::unistd::geteuid().as_raw() == 0
}

#[cfg(all(windows, not(feature = "test_in_root")))]
fn is_privileged() -> bool {
    is_elevated::is_elevated()
}

#[cfg(feature = "test_in_root")]
fn is_privileged() -> bool {
    true
}
async fn ensure_installed<E: Extractor>(
    target_dir: &Path,
    repo: &GitHubRepo,
    release: &Release,
    client: &Client,
    extractor: &E,
) -> Result<()> {
    if target_dir.exists() {
        info!(
            "Directory {:?} already exists. Skipping download and extraction.",
            target_dir
        );
        return Ok(());
    }

    debug!("Creating target directory: {:?}", target_dir);
    fs::create_dir_all(target_dir)
        .with_context(|| format!("Failed to create target directory at {:?}", target_dir))?;

    let temp_dir = std::env::temp_dir();
    let temp_file_path = temp_dir.join(format!("{}-{}.tar.gz", repo.repo, release.tag_name));

    download_file(&release.tarball_url, &temp_file_path, client).await?;
    extractor.extract(&temp_file_path, target_dir)?;

    fs::remove_file(&temp_file_path)
        .with_context(|| format!("Failed to clean up temporary file: {:?}", temp_file_path))?;

    Ok(())
}

fn update_current_symlink(target_dir: &Path, tag_name: &str) -> Result<()> {
    let current_link = target_dir
        .parent()
        .expect("target_dir must have a parent")
        .join("current");

    let link_target = target_dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Failed to get target directory name"))?;

    let mut create_symlink = true;

    match fs::symlink_metadata(&current_link) {
        Ok(metadata) => {
            if metadata.is_symlink() {
                let current_dest = fs::read_link(&current_link).with_context(|| {
                    format!("Failed to read symlink target of {:?}", current_link)
                })?;

                if current_dest == Path::new(link_target) {
                    info!("Symlink 'current' already points to version {}", tag_name);
                    create_symlink = false;
                } else {
                    #[cfg(not(windows))]
                    fs::remove_file(&current_link).with_context(|| {
                        format!("Failed to remove existing symlink at {:?}", current_link)
                    })?;
                    #[cfg(windows)]
                    fs::remove_dir(&current_link).with_context(|| {
                        format!("Failed to remove existing symlink at {:?}", current_link)
                    })?;
                }
            } else {
                bail!("Path {:?} exists but is not a symlink", current_link);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No processing needed if it doesn't exist
        }
        Err(e) => {
            return Err(e).context(format!("Failed to read metadata for {:?}", current_link));
        }
    }

    if create_symlink {
        #[cfg(unix)]
        std::os::unix::fs::symlink(link_target, &current_link)
            .with_context(|| format!("Failed to update 'current' symlink to {:?}", target_dir))?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(link_target, &current_link)
            .with_context(|| format!("Failed to update 'current' symlink to {:?}", target_dir))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn test_get_target_dir() {
        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
        };
        let release = Release {
            tag_name: "v1.0.0".to_string(),
            tarball_url: "http://example.com".to_string(),
            name: Some("v1.0.0".to_string()),
            published_at: Some("2020-01-01T00:00:00Z".to_string()),
            prerelease: false,
            assets: vec![],
        };

        let target_dir = get_target_dir(&repo, &release, None).unwrap();
        assert!(target_dir.ends_with("owner/repo/v1.0.0"));
    }

    #[test]
    fn test_get_target_dir_with_custom_root() {
        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
        };
        let release = Release {
            tag_name: "v1.0.0".to_string(),
            tarball_url: "http://example.com".to_string(),
            name: Some("v1.0.0".to_string()),
            published_at: Some("2020-01-01T00:00:00Z".to_string()),
            prerelease: false,
            assets: vec![],
        };

        let custom_root = tempdir().unwrap();
        let target_dir =
            get_target_dir(&repo, &release, Some(custom_root.path().to_path_buf())).unwrap();

        assert!(target_dir.starts_with(custom_root.path()));
        assert!(target_dir.ends_with("owner/repo/v1.0.0"));
    }

    #[test]
    fn test_update_current_symlink_create_new() {
        let dir = tempdir().unwrap();
        let base_path = dir.path().join("owner/repo");
        fs::create_dir_all(&base_path).unwrap();

        let target_ver = base_path.join("v1.0.0");
        fs::create_dir(&target_ver).unwrap();

        update_current_symlink(&target_ver, "v1.0.0").unwrap();

        let link_path = base_path.join("current");
        assert!(link_path.is_symlink());
        assert_eq!(fs::read_link(&link_path).unwrap(), Path::new("v1.0.0"));
    }

    #[test]
    fn test_update_current_symlink_update_existing() {
        let dir = tempdir().unwrap();
        let base_path = dir.path().join("owner/repo");
        fs::create_dir_all(&base_path).unwrap();

        let v1 = base_path.join("v1.0.0");
        let v2 = base_path.join("v2.0.0");
        fs::create_dir(&v1).unwrap();
        fs::create_dir(&v2).unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink("v1.0.0", base_path.join("current")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir("v1.0.0", base_path.join("current")).unwrap();

        update_current_symlink(&v2, "v2.0.0").unwrap();

        let link_path = base_path.join("current");
        assert!(link_path.is_symlink());
        assert_eq!(fs::read_link(&link_path).unwrap(), Path::new("v2.0.0"));
    }

    #[test]
    fn test_update_current_symlink_idempotent() {
        let dir = tempdir().unwrap();
        let base_path = dir.path().join("owner/repo");
        fs::create_dir_all(&base_path).unwrap();

        let v1 = base_path.join("v1.0.0");
        fs::create_dir(&v1).unwrap();

        update_current_symlink(&v1, "v1.0.0").unwrap();
        update_current_symlink(&v1, "v1.0.0").unwrap();

        let link_path = base_path.join("current");
        assert!(link_path.is_symlink());
        assert_eq!(fs::read_link(&link_path).unwrap(), Path::new("v1.0.0"));
    }

    #[test]
    fn test_update_current_symlink_fails_if_not_symlink() {
        let dir = tempdir().unwrap();
        let base_path = dir.path().join("owner/repo");
        fs::create_dir_all(&base_path).unwrap();

        let v1 = base_path.join("v1.0.0");
        fs::create_dir(&v1).unwrap();

        let current_path = base_path.join("current");
        File::create(&current_path).unwrap();

        let result = update_current_symlink(&v1, "v1.0.0");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exists but is not a symlink")
        );
    }

    #[test]
    fn test_update_current_symlink_no_op_if_already_correct() {
        let dir = tempdir().unwrap();
        let base_path = dir.path().join("owner/repo");
        fs::create_dir_all(&base_path).unwrap();
        let target_ver = base_path.join("v1.0.0");
        fs::create_dir(&target_ver).unwrap();
        let link_path = base_path.join("current");
        #[cfg(unix)]
        std::os::unix::fs::symlink("v1.0.0", &link_path).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir("v1.0.0", &link_path).unwrap();

        let metadata_before = fs::symlink_metadata(&link_path).unwrap();
        update_current_symlink(&target_ver, "v1.0.0").unwrap();
        let metadata_after = fs::symlink_metadata(&link_path).unwrap();

        assert_eq!(metadata_before.modified().unwrap(), metadata_after.modified().unwrap());
    }

    #[test]
    #[cfg(feature = "test_in_root")]
    fn test_default_install_root_privileged() {
        let root = default_install_root().unwrap();
        assert_eq!(root, system_install_root());
    }

    struct MockGitHub {
        release: Release,
    }

    #[async_trait]
    impl GetReleases for MockGitHub {
        async fn get_latest_release(&self, _repo: &GitHubRepo) -> Result<Release> {
            Ok(self.release.clone())
        }

        async fn get_repo_info(&self, _repo: &GitHubRepo) -> Result<RepoInfo> {
            Ok(RepoInfo {
                description: Some("description".into()),
                homepage: Some("homepage".into()),
                license: Some(crate::github::License {
                    key: "mit".into(),
                    name: "MIT".into(),
                }),
                updated_at: "2020-01-01T00:00:00Z".into(),
            })
        }

        async fn get_releases(&self, _repo: &GitHubRepo) -> Result<Vec<Release>> {
            Ok(vec![self.release.clone()])
        }
    }

    struct MockExtractor;

    impl Extractor for MockExtractor {
        fn extract(&self, _archive_path: &Path, extract_to: &Path) -> Result<()> {
            fs::create_dir_all(extract_to)?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_install() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo-install".to_string(),
        };

        let release = Release {
            tag_name: "v1.0.0".to_string(),
            tarball_url: format!("{}/download", url),
            name: Some("v1.0.0".to_string()),
            published_at: Some("2020-01-01T00:00:00Z".to_string()),
            prerelease: false,
            assets: vec![],
        };

        let mock_github = MockGitHub {
            release: release.clone(),
        };

        let client = Client::new();
        let mock_extractor = MockExtractor;
        let installer = Installer::new(mock_github, client, mock_extractor);

        let root_dir = tempdir().unwrap();
        let install_root = root_dir.path().to_path_buf();

        let _m = server
            .mock("GET", "/download")
            .with_status(200)
            .with_body("test")
            .create();

        installer
            .install(&repo, Some(install_root.clone()))
            .await
            .unwrap();

        let target_dir = install_root.join("owner/repo-install/v1.0.0");
        assert!(target_dir.exists());

        let current_link = install_root.join("owner/repo-install/current");
        assert!(current_link.is_symlink());
        assert_eq!(fs::read_link(&current_link).unwrap(), Path::new("v1.0.0"));

        let meta_file = install_root.join("owner/repo-install/meta.json");
        assert!(meta_file.exists());
        let meta_content = fs::read_to_string(meta_file).unwrap();
        assert!(meta_content.contains("v1.0.0"));
        assert!(meta_content.contains("owner/repo-install"));
    }

    #[tokio::test]
    async fn test_install_save_metadata_fails() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo-metadata-fails".to_string(),
        };

        let release = Release {
            tag_name: "v1.0.0".to_string(),
            tarball_url: format!("{}/download", url),
            ..Default::default()
        };

        struct MockGitHubFails {
            release: Release,
        }

        #[async_trait]
        impl GetReleases for MockGitHubFails {
            async fn get_latest_release(&self, _repo: &GitHubRepo) -> Result<Release> {
                Ok(self.release.clone())
            }

            async fn get_repo_info(&self, _repo: &GitHubRepo) -> Result<RepoInfo> {
                Err(anyhow::anyhow!("Failed to get repo info"))
            }

            async fn get_releases(&self, _repo: &GitHubRepo) -> Result<Vec<Release>> {
                Ok(vec![self.release.clone()])
            }
        }

        let mock_github = MockGitHubFails {
            release: release.clone(),
        };
        let client = Client::new();
        let mock_extractor = MockExtractor;
        let installer = Installer::new(mock_github, client, mock_extractor);

        let root_dir = tempdir().unwrap();
        let install_root = root_dir.path().to_path_buf();

        let _m = server
            .mock("GET", "/download")
            .with_status(200)
            .with_body("test")
            .create();

        // This should not panic, even though save_metadata fails.
        installer
            .install(&repo, Some(install_root.clone()))
            .await
            .unwrap();

        // Verify that the installation still completed
        let target_dir = install_root.join("owner/repo-metadata-fails/v1.0.0");
        assert!(target_dir.exists());

        // Verify that the metadata file was not created
        let meta_file = install_root.join("owner/repo-metadata-fails/meta.json");
        assert!(!meta_file.exists());
    }

    #[tokio::test]
    async fn test_run() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let repo_str = "owner/repo-run";
        let release = Release {
            tag_name: "v1.0.0".to_string(),
            tarball_url: format!("{}/download", url),
            name: Some("v1.0.0".to_string()),
            published_at: Some("2020-01-01T00:00:00Z".to_string()),
            prerelease: false,
            assets: vec![],
        };

        let mock_github = MockGitHub {
            release: release.clone(),
        };

        let client = Client::new();
        let mock_extractor = MockExtractor;

        let root_dir = tempdir().unwrap();
        let install_root = root_dir.path().to_path_buf();

        let config = Config {
            github: mock_github,
            client,
            extractor: mock_extractor,
            install_root: Some(install_root.clone()),
        };

        let _m = server
            .mock("GET", "/download")
            .with_status(200)
            .with_body("test")
            .create();

        run(repo_str, config).await.unwrap();

        let target_dir = install_root.join("owner/repo-run/v1.0.0");
        assert!(target_dir.exists());

        let current_link = install_root.join("owner/repo-run/current");
        assert!(current_link.is_symlink());
        assert_eq!(fs::read_link(&current_link).unwrap(), Path::new("v1.0.0"));

        let meta_file = install_root.join("owner/repo-run/meta.json");
        assert!(meta_file.exists());
        let meta_content = fs::read_to_string(meta_file).unwrap();
        assert!(meta_content.contains("v1.0.0"));
        assert!(meta_content.contains("owner/repo-run"));
    }

    #[tokio::test]
    async fn test_run_invalid_repo_str() {
        let repo_str = "invalid-repo-str";

        let release = Release {
            tag_name: "v1.0.0".to_string(),
            tarball_url: "http://example.com/download".to_string(),
            name: Some("v1.0.0".to_string()),
            published_at: Some("2020-01-01T00:00:00Z".to_string()),
            prerelease: false,
            assets: vec![],
        };

        let mock_github = MockGitHub { release };
        let client = Client::new();
        let mock_extractor = MockExtractor;

        let config = Config {
            github: mock_github,
            client,
            extractor: mock_extractor,
            install_root: None,
        };

        let result = run(repo_str, config).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid repository format"));
    }

    #[tokio::test]
    async fn test_ensure_installed_already_exists() {
        let dir = tempdir().unwrap();
        let target_dir = dir.path().join("owner/repo/v1.0.0");
        fs::create_dir_all(&target_dir).unwrap();

        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
        };
        let release = Release {
            tag_name: "v1.0.0".to_string(),
            tarball_url: "http://example.com".to_string(),
            name: Some("v1.0.0".to_string()),
            published_at: Some("2020-01-01T00:00:00Z".to_string()),
            prerelease: false,
            assets: vec![],
        };

        struct FailExtractor;
        impl Extractor for FailExtractor {
            fn extract(&self, _archive_path: &Path, _extract_to: &Path) -> Result<()> {
                panic!("should not be called");
            }
        }

        let client = Client::new();
        ensure_installed(&target_dir, &repo, &release, &client, &FailExtractor)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_ensure_installed_creates_dir_and_extracts() {
        let mut server = mockito::Server::new_async().await;
        let url = server.url();

        let dir = tempdir().unwrap();
        let target_dir = dir.path().join("owner/repo/v1.0.0");

        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
        };
        let release = Release {
            tag_name: "v1.0.0".to_string(),
            tarball_url: format!("{}/download", url),
            ..Default::default()
        };

        let mock_extractor = MockExtractor;

        let _m = server
            .mock("GET", "/download")
            .with_status(200)
            .with_body("test")
            .create();

        let client = Client::new();
        ensure_installed(&target_dir, &repo, &release, &client, &mock_extractor)
            .await
            .unwrap();

        assert!(target_dir.exists());
    }
}
