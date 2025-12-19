use crate::{
    archive::Extractor,
    download::download_file,
    github::{GetReleases, GitHubRepo, Release, ReleaseAsset, RepoInfo},
};
use anyhow::{Context, Result, bail};
use log::{debug, info, warn};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub mod config;
use config::Config;

pub async fn install(
    repo_str: &str,
    install_root: Option<PathBuf>,
    api_url: Option<String>,
) -> Result<()> {
    let config = Config::new(install_root, api_url)?;
    run(repo_str, config).await
}

pub async fn run<G: GetReleases, E: Extractor>(repo_str: &str, config: Config<G, E>) -> Result<()> {
    let repo = repo_str.parse::<GitHubRepo>()?;
    let installer = Installer::new(config.github, config.client, config.extractor);
    installer.install(&repo, config.install_root).await
}

pub async fn update(install_root: Option<PathBuf>, api_url: Option<String>) -> Result<()> {
    let config = Config::new(install_root, api_url)?;
    let installer = Installer::new(config.github, config.client, config.extractor);
    installer.update_all(config.install_root).await
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
        println!("   resolving {}", repo);
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

    pub async fn update_all(&self, install_root: Option<PathBuf>) -> Result<()> {
        let root = match install_root {
            Some(path) => path,
            None => default_install_root()?,
        };

        let meta_files = find_all_packages(&root)?;
        if meta_files.is_empty() {
            println!("No packages installed.");
            return Ok(());
        }

        for meta_path in meta_files {
            let meta = Meta::load(&meta_path)?;
            let repo = meta.name.parse::<GitHubRepo>()?;

            println!("   updating {}", repo);
            if let Err(e) = self
                .save_metadata(&repo, &meta.current_version, &meta_path)
                .await
            {
                warn!("Failed to update metadata for {}: {}", repo, e);
            }
        }

        Ok(())
    }

    fn print_install_success(&self, repo: &GitHubRepo, tag: &str, target_dir: &Path) {
        println!("   installed {} {} {}", repo, tag, target_dir.display());
    }

    async fn save_metadata(
        &self,
        repo: &GitHubRepo,
        current_version: &str,
        target_dir: &Path,
    ) -> Result<()> {
        let meta_path = if target_dir.is_file() {
            target_dir.to_path_buf()
        } else {
            let package_root = target_dir.parent().context("Failed to get package root")?;
            package_root.join("meta.json")
        };

        let repo_info = self.github.get_repo_info(repo).await?;
        let releases = self.github.get_releases(repo).await?;
        let new_meta = Meta::from(repo.clone(), repo_info, releases, current_version);

        let mut final_meta = if meta_path.exists() {
            let mut existing = Meta::load(&meta_path)?;
            if existing.merge(new_meta.clone()) {
                existing.updated_at = new_meta.updated_at;
            }
            existing
        } else {
            new_meta
        };

        // Ensure current_version is always correct (e.g. if we just installed a new version)
        final_meta.current_version = current_version.to_string();

        let json = serde_json::to_string_pretty(&final_meta)?;
        let tmp_path = meta_path.with_extension("json.tmp");

        fs::write(&tmp_path, json).context("Failed to write temporary meta file")?;
        fs::rename(&tmp_path, &meta_path).context("Failed to atomically update meta.json")?;

        Ok(())
    }
}

fn find_all_packages(root: &Path) -> Result<Vec<PathBuf>> {
    let mut meta_files = Vec::new();

    if !root.exists() {
        return Ok(meta_files);
    }

    // Root structure: <root>/<owner>/<repo>/meta.json
    for owner_entry in fs::read_dir(root)? {
        let owner_path = owner_entry?.path();
        if owner_path.is_dir() {
            for repo_entry in fs::read_dir(owner_path)? {
                let repo_path = repo_entry?.path();
                if repo_path.is_dir() {
                    let meta_path = repo_path.join("meta.json");
                    if meta_path.exists() {
                        meta_files.push(meta_path);
                    }
                }
            }
        }
    }

    Ok(meta_files)
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
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

    fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let meta: Meta = serde_json::from_str(&content)?;
        Ok(meta)
    }

    fn merge(&mut self, other: Meta) -> bool {
        let mut changed = false;

        if self.description != other.description {
            self.description = other.description;
            changed = true;
        }
        if self.homepage != other.homepage {
            self.homepage = other.homepage;
            changed = true;
        }
        if self.license != other.license {
            self.license = other.license;
            changed = true;
        }

        for new_release in other.releases {
            if let Some(existing) = self
                .releases
                .iter_mut()
                .find(|r| r.version == new_release.version)
            {
                if existing != &new_release {
                    *existing = new_release;
                    changed = true;
                }
            } else {
                self.releases.push(new_release);
                changed = true;
            }
        }

        changed
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
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

    println!(" downloading {} {}", &repo, release.tag_name);
    download_file(&release.tarball_url, &temp_file_path, client).await?;

    println!("  installing {} {}", &repo, release.tag_name);
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

        assert_eq!(
            metadata_before.modified().unwrap(),
            metadata_after.modified().unwrap()
        );
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
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Invalid repository format")
        );
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

    #[test]
    fn test_find_all_packages() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        let repo1_meta = root.join("owner1/repo1/meta.json");
        fs::create_dir_all(repo1_meta.parent().unwrap()).unwrap();
        fs::write(&repo1_meta, "{}").unwrap();

        let repo2_meta = root.join("owner2/repo2/meta.json");
        fs::create_dir_all(repo2_meta.parent().unwrap()).unwrap();
        fs::write(&repo2_meta, "{}").unwrap();

        let packages = find_all_packages(root).unwrap();
        assert_eq!(packages.len(), 2);
        assert!(packages.contains(&repo1_meta));
        assert!(packages.contains(&repo2_meta));
    }

    #[tokio::test]
    async fn test_update_all() {
        let mut server = mockito::Server::new_async().await;

        let dir = tempdir().unwrap();
        let root = dir.path();

        let meta_path = root.join("owner/repo/meta.json");
        fs::create_dir_all(meta_path.parent().unwrap()).unwrap();

        let meta = Meta {
            name: "owner/repo".to_string(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "2020-01-01T00:00:00Z".to_string(),
            current_version: "v1.0.0".to_string(),
            releases: vec![],
        };
        fs::write(&meta_path, serde_json::to_string(&meta).unwrap()).unwrap();

        let mock_github = MockGitHub {
            release: Release {
                tag_name: "v1.0.0".to_string(),
                tarball_url: "url".to_string(),
                ..Default::default()
            },
        };

        let client = Client::new();
        let mock_extractor = MockExtractor;
        let installer = Installer::new(mock_github, client, mock_extractor);

        // Mock GitHub API calls (save_metadata calls these)
        let _m1 = server
            .mock("GET", "/repos/owner/repo")
            .with_status(200)
            .with_body(r#"{"updated_at": "2023-01-01T00:00:00Z"}"#)
            .create();
        let _m2 = server
            .mock("GET", "/repos/owner/repo/releases?per_page=100&page=1")
            .with_status(200)
            .with_body("[]")
            .create();

        installer
            .update_all(Some(root.to_path_buf()))
            .await
            .unwrap();

        let updated_meta = Meta::load(&meta_path).unwrap();
        assert_eq!(updated_meta.name, "owner/repo");
        // MockGitHub returns "2020-01-01T00:00:00Z" for updated_at in its get_repo_info impl
        assert_eq!(updated_meta.updated_at, "2020-01-01T00:00:00Z");
    }

    #[tokio::test]
    async fn test_update_merges_and_preserves_history() {
        let mut server = mockito::Server::new_async().await;
        let dir = tempdir().unwrap();
        let root = dir.path();
        let meta_path = root.join("owner/repo/meta.json");
        fs::create_dir_all(meta_path.parent().unwrap()).unwrap();

        // 1. Initial state with one release
        let initial_meta = Meta {
            name: "owner/repo".to_string(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "old-timestamp".to_string(),
            current_version: "v1.0.0".to_string(),
            releases: vec![MetaRelease {
                version: "v1.0.0".to_string(),
                title: Some("v1.0.0".to_string()),
                published_at: Some("2020-01-01T00:00:00Z".to_string()),
                is_prerelease: false,
                tarball_url: "url1".to_string(),
                assets: vec![],
            }],
        };
        fs::write(&meta_path, serde_json::to_string(&initial_meta).unwrap()).unwrap();

        // 2. Mock GitHub returns ONLY a NEW release (v2.0.0)
        let _m1 = server
            .mock("GET", "/repos/owner/repo")
            .with_status(200)
            .with_body(r#"{"updated_at": "new-timestamp"}"#)
            .create();
        let _m2 = server
            .mock("GET", "/repos/owner/repo/releases?per_page=100&page=1")
            .with_status(200)
            .with_body(r#"[{"tag_name": "v2.0.0", "tarball_url": "url2", "prerelease": false, "assets": []}]"#)
            .create();

        let github = crate::github::GitHub::new(reqwest::Client::new(), Some(server.url()));
        let installer = Installer::new(github, reqwest::Client::new(), MockExtractor);

        installer
            .update_all(Some(root.to_path_buf()))
            .await
            .unwrap();

        let updated_meta = Meta::load(&meta_path).unwrap();
        assert_eq!(updated_meta.releases.len(), 2);
        assert!(updated_meta.releases.iter().any(|r| r.version == "v1.0.0"));
        assert!(updated_meta.releases.iter().any(|r| r.version == "v2.0.0"));
        assert_eq!(updated_meta.updated_at, "new-timestamp");
    }

    #[tokio::test]
    async fn test_update_timestamp_behavior() {
        let mut server = mockito::Server::new_async().await;
        let dir = tempdir().unwrap();
        let root = dir.path();
        let meta_path = root.join("owner/repo/meta.json");
        fs::create_dir_all(meta_path.parent().unwrap()).unwrap();

        let initial_meta = Meta {
            name: "owner/repo".to_string(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "2020-01-01T00:00:00Z".to_string(),
            current_version: "v1.0.0".to_string(),
            releases: vec![MetaRelease {
                version: "v1.0.0".to_string(),
                title: None,
                published_at: None,
                is_prerelease: false,
                tarball_url: "url".to_string(),
                assets: vec![],
            }],
        };
        fs::write(&meta_path, serde_json::to_string(&initial_meta).unwrap()).unwrap();

        // Mock returns SAME data but DIFFERENT updated_at
        let _m1 = server
            .mock("GET", "/repos/owner/repo")
            .with_status(200)
            .with_body(r#"{"updated_at": "2023-01-01T00:00:00Z"}"#)
            .create();
        let _m2 = server
            .mock("GET", "/repos/owner/repo/releases?per_page=100&page=1")
            .with_status(200)
            .with_body(r#"[{"tag_name": "v1.0.0", "tarball_url": "url", "prerelease": false, "assets": []}]"#)
            .create();

        let github = crate::github::GitHub::new(reqwest::Client::new(), Some(server.url()));
        let installer = Installer::new(github, reqwest::Client::new(), MockExtractor);

        installer
            .update_all(Some(root.to_path_buf()))
            .await
            .unwrap();

        let updated_meta = Meta::load(&meta_path).unwrap();
        // Since content didn't change (v1.0.0 is same), updated_at should NOT change
        assert_eq!(updated_meta.updated_at, "2020-01-01T00:00:00Z");
    }

    #[tokio::test]
    async fn test_update_atomic_safety() {
        let mut server = mockito::Server::new_async().await;
        let dir = tempdir().unwrap();
        let root = dir.path();
        let meta_path = root.join("owner/repo/meta.json");
        fs::create_dir_all(meta_path.parent().unwrap()).unwrap();

        let initial_content = r#"{"name": "owner/repo", "current_version": "v1.0.0", "releases": [], "updated_at": "old"}"#;
        fs::write(&meta_path, initial_content).unwrap();

        // Mock returns 500 error to simulate failure midway
        let _m = server
            .mock("GET", "/repos/owner/repo")
            .with_status(500)
            .create();

        let github = crate::github::GitHub::new(reqwest::Client::new(), Some(server.url()));
        let installer = Installer::new(github, reqwest::Client::new(), MockExtractor);

        let result = installer.update_all(Some(root.to_path_buf())).await;
        assert!(result.is_ok());

        // Verify meta.json remains intact and NO .tmp files left
        let content = fs::read_to_string(&meta_path).unwrap();
        assert_eq!(content, initial_content);

        let mut tmp_exists = false;
        for entry in fs::read_dir(meta_path.parent().unwrap()).unwrap() {
            if entry
                .unwrap()
                .file_name()
                .to_str()
                .unwrap()
                .ends_with(".tmp")
            {
                tmp_exists = true;
            }
        }
        assert!(!tmp_exists);
    }
}
