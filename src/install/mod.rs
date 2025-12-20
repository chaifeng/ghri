use anyhow::{Context, Result, bail};
use log::{debug, info, warn};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::{
    archive::Extractor,
    download::download_file,
    github::{GetReleases, GitHubRepo, Release, ReleaseAsset, RepoInfo},
    runtime::Runtime,
};

pub mod config;
use config::Config;

pub async fn install<R: Runtime>(
    runtime: R,
    repo_str: &str,
    install_root: Option<PathBuf>,
    api_url: Option<String>,
) -> Result<()> {
    let config = Config::new(runtime, install_root, api_url)?;
    run(repo_str, config).await
}

pub async fn run<R: Runtime, G: GetReleases, E: Extractor>(
    repo_str: &str,
    config: Config<R, G, E>,
) -> Result<()> {
    let repo = repo_str.parse::<GitHubRepo>()?;
    let installer = Installer::new(
        config.runtime,
        config.github,
        config.client,
        config.extractor,
    );
    installer.install(&repo, config.install_root).await
}

pub async fn update<R: Runtime>(
    runtime: R,
    install_root: Option<PathBuf>,
    api_url: Option<String>,
) -> Result<()> {
    let config = Config::new(runtime, install_root, api_url)?;
    let installer = Installer::new(
        config.runtime,
        config.github,
        config.client,
        config.extractor,
    );
    installer.update_all(config.install_root).await
}

pub struct Installer<R: Runtime, G: GetReleases, E: Extractor> {
    pub runtime: R,
    pub github: G,
    pub client: Client,
    pub extractor: E,
}

impl<R: Runtime, G: GetReleases, E: Extractor> Installer<R, G, E> {
    pub fn new(runtime: R, github: G, client: Client, extractor: E) -> Self {
        Self {
            runtime,
            github,
            client,
            extractor,
        }
    }

    pub async fn install(&self, repo: &GitHubRepo, install_root: Option<PathBuf>) -> Result<()> {
        println!("   resolving {}", repo);
        let (mut meta, meta_path) = self
            .get_or_fetch_meta(repo, install_root.as_deref())
            .await?;

        let meta_release = meta
            .get_latest_stable_release()
            .ok_or_else(|| anyhow::anyhow!("No stable release found for {}. If you want to install a pre-release, please use the update command or specify the version (not yet supported).", repo))?;

        info!("Found latest version: {}", meta_release.version);
        let release: Release = meta_release.clone().into();

        let target_dir = get_target_dir(&self.runtime, &repo, &release, install_root)?;

        ensure_installed(
            &self.runtime,
            &target_dir,
            &repo,
            &release,
            &self.client,
            &self.extractor,
        )
        .await?;
        update_current_symlink(&self.runtime, &target_dir, &release.tag_name)?;

        // Metadata handling
        meta.current_version = release.tag_name.clone();
        if let Err(e) = self.save_meta(&meta_path, &meta) {
            warn!("Failed to save package metadata: {}. Continuing.", e);
        }

        self.print_install_success(repo, &release.tag_name, &target_dir);

        Ok(())
    }

    pub async fn update_all(&self, install_root: Option<PathBuf>) -> Result<()> {
        let root = match install_root {
            Some(path) => path,
            None => default_install_root(&self.runtime)?,
        };

        let meta_files = find_all_packages(&self.runtime, &root)?;
        if meta_files.is_empty() {
            println!("No packages installed.");
            return Ok(());
        }

        for meta_path in meta_files {
            let meta = Meta::load(&self.runtime, &meta_path)?;
            let repo = meta.name.parse::<GitHubRepo>()?;

            println!("   updating {}", repo);
            if let Err(e) = self
                .save_metadata(&repo, &meta.current_version, &meta_path)
                .await
            {
                warn!("Failed to update metadata for {}: {}", repo, e);
            } else {
                // Check if update is available
                let updated_meta = Meta::load(&self.runtime, &meta_path)?;
                if let Some(latest) = updated_meta.get_latest_stable_release() {
                    if meta.current_version != latest.version {
                        self.print_update_available(&repo, &meta.current_version, &latest.version);
                    }
                }
            }
        }

        Ok(())
    }

    fn print_install_success(&self, repo: &GitHubRepo, tag: &str, target_dir: &Path) {
        println!("   installed {} {} {}", repo, tag, target_dir.display());
    }

    fn print_update_available(&self, repo: &GitHubRepo, current: &str, latest: &str) {
        let current_display = if current.is_empty() {
            "(none)"
        } else {
            current
        };
        println!("  updatable {} {} -> {}", repo, current_display, latest);
    }

    async fn get_or_fetch_meta(
        &self,
        repo: &GitHubRepo,
        install_root: Option<&Path>,
    ) -> Result<(Meta, PathBuf)> {
        let root = match install_root {
            Some(path) => path.to_path_buf(),
            None => default_install_root(&self.runtime)?,
        };
        let meta_path = root.join(&repo.owner).join(&repo.repo).join("meta.json");

        if self.runtime.exists(&meta_path) {
            match Meta::load(&self.runtime, &meta_path) {
                Ok(meta) => return Ok((meta, meta_path)),
                Err(e) => {
                    warn!(
                        "Failed to load existing meta.json at {:?}: {}. Re-fetching.",
                        meta_path, e
                    );
                }
            }
        }

        let meta = self.fetch_meta(repo, "", None).await?;

        if let Some(parent) = meta_path.parent() {
            self.runtime.create_dir_all(parent)?;
        }
        self.save_meta(&meta_path, &meta)?;

        Ok((meta, meta_path))
    }

    async fn fetch_meta(
        &self,
        repo: &GitHubRepo,
        current_version: &str,
        api_url: Option<&str>,
    ) -> Result<Meta> {
        let api_url = api_url.unwrap_or(self.github.api_url());
        let repo_info = self.github.get_repo_info_at(repo, api_url).await?;
        let releases = self.github.get_releases_at(repo, api_url).await?;
        Ok(Meta::from(
            repo.clone(),
            repo_info,
            releases,
            current_version,
            api_url,
        ))
    }

    fn save_meta(&self, meta_path: &Path, meta: &Meta) -> Result<()> {
        let json = serde_json::to_string_pretty(meta)?;
        let tmp_path = meta_path.with_extension("json.tmp");

        self.runtime.write(&tmp_path, json.as_bytes())?;
        self.runtime.rename(&tmp_path, &meta_path)?;
        Ok(())
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

        let existing_meta = if self.runtime.exists(&meta_path) {
            Meta::load(&self.runtime, &meta_path).ok()
        } else {
            None
        };

        let new_meta = self
            .fetch_meta(
                repo,
                current_version,
                existing_meta.as_ref().map(|m| m.api_url.as_str()),
            )
            .await?;

        let mut final_meta = if self.runtime.exists(&meta_path) {
            let mut existing = Meta::load(&self.runtime, &meta_path)?;
            if existing.merge(new_meta.clone()) {
                existing.updated_at = new_meta.updated_at;
            }
            existing
        } else {
            new_meta
        };

        // Ensure current_version is always correct (e.g. if we just installed a new version)
        final_meta.current_version = current_version.to_string();

        self.save_meta(&meta_path, &final_meta)?;

        Ok(())
    }
}

fn find_all_packages<R: Runtime>(runtime: &R, root: &Path) -> Result<Vec<PathBuf>> {
    let mut meta_files = Vec::new();

    if !runtime.exists(root) {
        return Ok(meta_files);
    }

    // Root structure: <root>/<owner>/<repo>/meta.json
    for owner_path in runtime.read_dir(root)? {
        if owner_path.is_dir() {
            for repo_path in runtime.read_dir(&owner_path)? {
                if repo_path.is_dir() {
                    let meta_path = repo_path.join("meta.json");
                    if runtime.exists(&meta_path) {
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
    api_url: String,
    repo_info_url: String,
    releases_url: String,
    description: Option<String>,
    homepage: Option<String>,
    license: Option<String>,
    updated_at: String,
    current_version: String,
    releases: Vec<MetaRelease>,
}

impl Meta {
    pub fn from(
        repo: GitHubRepo,
        info: RepoInfo,
        releases: Vec<Release>,
        current: &str,
        api_url: &str,
    ) -> Self {
        Meta {
            name: format!("{}/{}", repo.owner, repo.repo),
            api_url: api_url.to_string(),
            repo_info_url: format!("{}/repos/{}/{}", api_url, repo.owner, repo.repo),
            releases_url: format!("{}/repos/{}/{}/releases", api_url, repo.owner, repo.repo),
            description: info.description,
            homepage: info.homepage,
            license: info.license.map(|l| l.name),
            updated_at: info.updated_at,
            current_version: current.to_string(),
            releases: {
                let mut r: Vec<MetaRelease> = releases.into_iter().map(MetaRelease::from).collect();
                Meta::sort_releases_internal(&mut r);
                r
            },
        }
    }

    fn sort_releases_internal(releases: &mut [MetaRelease]) {
        releases.sort_by(|a, b| {
            match (&a.published_at, &b.published_at) {
                (Some(at_a), Some(at_b)) => at_b.cmp(at_a),  // Descending
                (Some(_), None) => std::cmp::Ordering::Less, // Published comes before unpublished
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => b.version.cmp(&a.version), // Version descending fallback
            }
        });
    }

    pub fn sort_releases(&mut self) {
        Self::sort_releases_internal(&mut self.releases);
    }

    pub fn get_latest_stable_release(&self) -> Option<&MetaRelease> {
        self.releases
            .iter()
            .filter(|r| !r.is_prerelease)
            .max_by(|a, b| {
                // Simplified version comparison: tag_name might not be semver-compliant,
                // but published_at is a good proxy for "latest".
                // If published_at is missing, fall back to version string comparison.
                match (&a.published_at, &b.published_at) {
                    (Some(at_a), Some(at_b)) => at_a.cmp(at_b),
                    _ => a.version.cmp(&b.version),
                }
            })
    }

    fn load<R: Runtime>(runtime: &R, path: &Path) -> Result<Self> {
        let content = runtime.read_to_string(path)?;
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

        if changed {
            self.sort_releases();
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

impl From<MetaRelease> for Release {
    fn from(r: MetaRelease) -> Self {
        Release {
            tag_name: r.version,
            tarball_url: r.tarball_url,
            name: r.title,
            published_at: r.published_at,
            prerelease: r.is_prerelease,
            assets: r.assets.into_iter().map(ReleaseAsset::from).collect(),
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

impl From<MetaAsset> for ReleaseAsset {
    fn from(a: MetaAsset) -> Self {
        ReleaseAsset {
            name: a.name,
            size: a.size,
            browser_download_url: a.download_url,
        }
    }
}

fn get_target_dir<R: Runtime>(
    runtime: &R,
    repo: &GitHubRepo,
    release: &Release,
    install_root: Option<PathBuf>,
) -> Result<PathBuf> {
    let root = match install_root {
        Some(path) => path,
        None => default_install_root(runtime)?,
    };

    info!("Using install root: {}", root.display());

    Ok(root
        .join(&repo.owner)
        .join(&repo.repo)
        .join(&release.tag_name))
}

fn default_install_root<R: Runtime>(runtime: &R) -> Result<PathBuf> {
    if is_privileged(runtime) {
        Ok(system_install_root(runtime))
    } else {
        let home_dir = runtime
            .home_dir()
            .context("Could not find home directory")?;
        Ok(home_dir.join(".ghri"))
    }
}

#[cfg(target_os = "macos")]
fn system_install_root<R: Runtime>(_runtime: &R) -> PathBuf {
    PathBuf::from("/opt/ghri")
}

#[cfg(target_os = "windows")]
fn system_install_root<R: Runtime>(runtime: &R) -> PathBuf {
    runtime
        .config_dir()
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData\ghri"))
        .join("ghri")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn system_install_root<R: Runtime>(_runtime: &R) -> PathBuf {
    PathBuf::from("/usr/local/ghri")
}

#[cfg(all(unix, not(feature = "test_in_root")))]
fn is_privileged<R: Runtime>(runtime: &R) -> bool {
    // Check if running as root via UID or USER env var
    match runtime.env_var("USER") {
        Ok(user) => user == "root",
        Err(_) => false,
    }
}

#[cfg(all(windows, not(feature = "test_in_root")))]
fn is_privileged<R: Runtime>(_runtime: &R) -> bool {
    // simplified for now as Runtime doesn't have is_elevated yet
    false
}

#[cfg(feature = "test_in_root")]
fn is_privileged<R: Runtime>(_runtime: &R) -> bool {
    true
}
async fn ensure_installed<R: Runtime, E: Extractor>(
    runtime: &R,
    target_dir: &Path,
    repo: &GitHubRepo,
    release: &Release,
    client: &Client,
    extractor: &E,
) -> Result<()> {
    if runtime.exists(target_dir) {
        info!(
            "Directory {:?} already exists. Skipping download and extraction.",
            target_dir
        );
        return Ok(());
    }

    debug!("Creating target directory: {:?}", target_dir);
    runtime
        .create_dir_all(target_dir)
        .with_context(|| format!("Failed to create target directory at {:?}", target_dir))?;

    let temp_dir = std::env::temp_dir();
    let temp_file_path = temp_dir.join(format!("{}-{}.tar.gz", repo.repo, release.tag_name));

    println!(" downloading {} {}", &repo, release.tag_name);
    download_file(runtime, &release.tarball_url, &temp_file_path, client).await?;

    println!("  installing {} {}", &repo, release.tag_name);
    extractor.extract(runtime, &temp_file_path, target_dir)?;

    runtime
        .remove_file(&temp_file_path)
        .with_context(|| format!("Failed to clean up temporary file: {:?}", temp_file_path))?;

    Ok(())
}

fn update_current_symlink<R: Runtime>(
    runtime: &R,
    target_dir: &Path,
    _tag_name: &str,
) -> Result<()> {
    let current_link = target_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Failed to get parent directory"))?
        .join("current");

    let link_target = target_dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Failed to get target directory name"))?;

    let mut create_symlink = true;

    if runtime.exists(&current_link) {
        if !runtime.is_symlink(&current_link) {
            bail!("'current' exists but is not a symlink");
        }

        match runtime.read_link(&current_link) {
            Ok(target) => {
                // Normalize paths for comparison to handle minor differences like trailing slashes
                let existing_target_path = Path::new(&target).components().as_path();
                let new_target_path = Path::new(link_target).components().as_path();

                if existing_target_path == new_target_path {
                    debug!("'current' symlink already points to the correct version");
                    create_symlink = false;
                } else {
                    debug!(
                        "'current' symlink points to {:?}, but should point to {:?}. Updating...",
                        existing_target_path, new_target_path
                    );
                    runtime.remove_symlink(&current_link)?;
                }
            }
            Err(_) => {
                debug!("'current' symlink is unreadable, recreating...");
                runtime.remove_symlink(&current_link)?;
            }
        }
    }

    if create_symlink {
        runtime
            .symlink(Path::new(link_target), &current_link)
            .with_context(|| format!("Failed to update 'current' symlink to {:?}", target_dir))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{MockRuntime, RealRuntime};
    use async_trait::async_trait;
    use std::fs::{self, File};
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

        let runtime = MockRuntime::new();
        let target_dir = get_target_dir(&runtime, &repo, &release, None).unwrap();
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
        let target_dir = get_target_dir(
            &RealRuntime,
            &repo,
            &release,
            Some(custom_root.path().to_path_buf()),
        )
        .unwrap();

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

        update_current_symlink(&RealRuntime, &target_ver, "v1.0.0").unwrap();

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

        update_current_symlink(&RealRuntime, &v2, "v2.0.0").unwrap();

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

        update_current_symlink(&RealRuntime, &v1, "v1.0.0").unwrap();
        update_current_symlink(&RealRuntime, &v1, "v1.0.0").unwrap();

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

        let result = update_current_symlink(&RealRuntime, &v1, "v1.0.0");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exists but is not a symlink")
        );
    }

    #[test]
    fn test_meta_get_latest_stable_release() {
        let meta = Meta {
            name: "owner/repo".to_string(),
            api_url: "https://api.github.com".to_string(),
            repo_info_url: "https://api.github.com/repos/owner/repo".to_string(),
            releases_url: "https://api.github.com/repos/owner/repo/releases".to_string(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "2023-01-01T00:00:00Z".to_string(),
            current_version: "v1.0.0".to_string(),
            releases: vec![
                MetaRelease {
                    version: "v1.0.0".to_string(),
                    title: None,
                    published_at: Some("2023-01-01T00:00:00Z".to_string()),
                    is_prerelease: false,
                    tarball_url: "url1".to_string(),
                    assets: vec![],
                },
                MetaRelease {
                    version: "v1.1.0-rc.1".to_string(),
                    title: None,
                    published_at: Some("2023-02-01T00:00:00Z".to_string()),
                    is_prerelease: true,
                    tarball_url: "url2".to_string(),
                    assets: vec![],
                },
                MetaRelease {
                    version: "v0.9.0".to_string(),
                    title: None,
                    published_at: Some("2022-12-01T00:00:00Z".to_string()),
                    is_prerelease: false,
                    tarball_url: "url3".to_string(),
                    assets: vec![],
                },
            ],
        };

        let latest = meta.get_latest_stable_release().unwrap();
        assert_eq!(latest.version, "v1.0.0");
    }

    #[test]
    fn test_meta_get_latest_stable_release_empty() {
        let meta = Meta {
            name: "owner/repo".to_string(),
            api_url: "https://api.github.com".to_string(),
            repo_info_url: "https://api.github.com/repos/owner/repo".to_string(),
            releases_url: "https://api.github.com/repos/owner/repo/releases".to_string(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "2023-01-01T00:00:00Z".to_string(),
            current_version: "".to_string(),
            releases: vec![],
        };
        assert!(meta.get_latest_stable_release().is_none());
    }

    #[test]
    fn test_meta_get_latest_stable_release_only_prerelease() {
        let meta = Meta {
            name: "owner/repo".to_string(),
            api_url: "https://api.github.com".to_string(),
            repo_info_url: "https://api.github.com/repos/owner/repo".to_string(),
            releases_url: "https://api.github.com/repos/owner/repo/releases".to_string(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "2023-01-01T00:00:00Z".to_string(),
            current_version: "".to_string(),
            releases: vec![MetaRelease {
                version: "v1.0.0-rc.1".to_string(),
                title: None,
                published_at: Some("2023-01-01T00:00:00Z".to_string()),
                is_prerelease: true,
                tarball_url: "url1".to_string(),
                assets: vec![],
            }],
        };
        assert!(meta.get_latest_stable_release().is_none());
    }

    #[test]
    fn test_meta_conversions() {
        let meta_asset = MetaAsset {
            name: "test.tar.gz".to_string(),
            size: 1234,
            download_url: "http://example.com/test.tar.gz".to_string(),
        };

        let asset: ReleaseAsset = meta_asset.clone().into();
        assert_eq!(asset.name, meta_asset.name);
        assert_eq!(asset.size, meta_asset.size);
        assert_eq!(asset.browser_download_url, meta_asset.download_url);

        let meta_release = MetaRelease {
            version: "v1.0.0".to_string(),
            title: Some("Release v1.0.0".to_string()),
            published_at: Some("2023-01-01T00:00:00Z".to_string()),
            is_prerelease: false,
            tarball_url: "http://example.com/v1.0.0.tar.gz".to_string(),
            assets: vec![meta_asset],
        };

        let release: Release = meta_release.clone().into();
        assert_eq!(release.tag_name, meta_release.version);
        assert_eq!(release.tarball_url, meta_release.tarball_url);
        assert_eq!(release.name, meta_release.title);
        assert_eq!(release.published_at, meta_release.published_at);
        assert_eq!(release.prerelease, meta_release.is_prerelease);
        assert_eq!(release.assets.len(), 1);
        assert_eq!(release.assets[0].name, "test.tar.gz");
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
        update_current_symlink(&RealRuntime, &target_ver, "v1.0.0").unwrap();
        let metadata_after = fs::symlink_metadata(&link_path).unwrap();

        assert_eq!(
            metadata_before.modified().unwrap(),
            metadata_after.modified().unwrap()
        );
    }

    #[test]
    #[cfg(feature = "test_in_root")]
    fn test_default_install_root_privileged() {
        let root = default_install_root(&RealRuntime).unwrap();
        assert_eq!(root, system_install_root(&RealRuntime));
    }

    struct MockGitHub {
        release: Release,
    }

    #[async_trait]
    impl GetReleases for MockGitHub {
        async fn get_repo_info(&self, repo: &GitHubRepo) -> Result<RepoInfo> {
            self.get_repo_info_at(repo, self.api_url()).await
        }

        async fn get_releases(&self, repo: &GitHubRepo) -> Result<Vec<Release>> {
            self.get_releases_at(repo, self.api_url()).await
        }

        async fn get_repo_info_at(&self, _repo: &GitHubRepo, _api_url: &str) -> Result<RepoInfo> {
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

        async fn get_releases_at(
            &self,
            _repo: &GitHubRepo,
            _api_url: &str,
        ) -> Result<Vec<Release>> {
            Ok(vec![self.release.clone()])
        }

        fn api_url(&self) -> &str {
            "https://api.github.com"
        }
    }

    struct MockExtractor;

    impl Extractor for MockExtractor {
        fn extract<R: Runtime>(
            &self,
            runtime: &R,
            _archive_path: &Path,
            extract_to: &Path,
        ) -> Result<()> {
            runtime.create_dir_all(extract_to)?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_get_or_fetch_meta_missing() {
        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo-missing".to_string(),
        };

        let release = Release {
            tag_name: "v1.0.0".to_string(),
            tarball_url: "url".to_string(),
            ..Default::default()
        };

        let mock_github = MockGitHub {
            release: release.clone(),
        };

        let client = Client::new();
        let installer = Installer::new(RealRuntime, mock_github, client, MockExtractor);

        let root_dir = tempdir().unwrap();
        let install_root = root_dir.path().to_path_buf();

        let (meta, path) = installer
            .get_or_fetch_meta(&repo, Some(&install_root))
            .await
            .unwrap();

        assert_eq!(meta.name, "owner/repo-missing");
        assert!(path.exists());
        assert!(path.ends_with("owner/repo-missing/meta.json"));

        let loaded = Meta::load(&RealRuntime, &path).unwrap();
        assert_eq!(loaded, meta);
    }

    #[tokio::test]
    async fn test_get_or_fetch_meta_exists() {
        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo-exists".to_string(),
        };

        let root_dir = tempdir().unwrap();
        let install_root = root_dir.path().to_path_buf();
        let meta_dir = install_root.join("owner/repo-exists");
        fs::create_dir_all(&meta_dir).unwrap();
        let meta_path = meta_dir.join("meta.json");

        let existing_meta = Meta {
            name: "owner/repo-exists".to_string(),
            api_url: "https://api.github.com".to_string(),
            repo_info_url: "https://api.github.com/repos/owner/repo-exists".to_string(),
            releases_url: "https://api.github.com/repos/owner/repo-exists/releases".to_string(),
            description: Some("old description".into()),
            homepage: None,
            license: None,
            updated_at: "2023-01-01T00:00:00Z".to_string(),
            current_version: "v0.1.0".to_string(),
            releases: vec![],
        };
        let json = serde_json::to_string_pretty(&existing_meta).unwrap();
        fs::write(&meta_path, json).unwrap();

        struct PanicGitHub;
        #[async_trait]
        impl GetReleases for PanicGitHub {
            async fn get_repo_info(&self, _: &GitHubRepo) -> Result<RepoInfo> {
                panic!("Should not be called")
            }
            async fn get_releases(&self, _: &GitHubRepo) -> Result<Vec<Release>> {
                panic!("Should not be called")
            }
            async fn get_repo_info_at(&self, _: &GitHubRepo, _: &str) -> Result<RepoInfo> {
                panic!("Should not be called")
            }
            async fn get_releases_at(&self, _: &GitHubRepo, _: &str) -> Result<Vec<Release>> {
                panic!("Should not be called")
            }
            fn api_url(&self) -> &str {
                "https://api.github.com"
            }
        }

        let installer = Installer::new(RealRuntime, PanicGitHub, Client::new(), MockExtractor);

        let (meta, path) = installer
            .get_or_fetch_meta(&repo, Some(&install_root))
            .await
            .unwrap();

        assert_eq!(meta, existing_meta);
        assert_eq!(path, meta_path);
    }

    #[tokio::test]
    async fn test_get_or_fetch_meta_invalid_on_disk() {
        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo-invalid".to_string(),
        };

        let root_dir = tempdir().unwrap();
        let install_root = root_dir.path().to_path_buf();
        let meta_dir = install_root.join("owner/repo-invalid");
        fs::create_dir_all(&meta_dir).unwrap();
        let meta_path = meta_dir.join("meta.json");
        fs::write(&meta_path, "invalid json").unwrap();

        let release = Release {
            tag_name: "v1.0.0".to_string(),
            tarball_url: "url".to_string(),
            ..Default::default()
        };
        let mock_github = MockGitHub {
            release: release.clone(),
        };

        let installer = Installer::new(RealRuntime, mock_github, Client::new(), MockExtractor);

        let (meta, path) = installer
            .get_or_fetch_meta(&repo, Some(&install_root))
            .await
            .unwrap();

        assert_eq!(meta.name, "owner/repo-invalid");
        assert!(path.exists());
        // Verify it was overwritten with valid data
        let loaded = Meta::load(&RealRuntime, &path).unwrap();
        assert_eq!(loaded, meta);
    }

    #[tokio::test]
    async fn test_get_or_fetch_meta_fetch_fail() {
        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo-fail".to_string(),
        };

        struct FailGitHub;
        #[async_trait]
        impl GetReleases for FailGitHub {
            async fn get_repo_info(&self, _: &GitHubRepo) -> Result<RepoInfo> {
                Err(anyhow::anyhow!("API Error"))
            }
            async fn get_releases(&self, _: &GitHubRepo) -> Result<Vec<Release>> {
                Err(anyhow::anyhow!("API Error"))
            }
            async fn get_repo_info_at(&self, _: &GitHubRepo, _: &str) -> Result<RepoInfo> {
                Err(anyhow::anyhow!("API Error"))
            }
            async fn get_releases_at(&self, _: &GitHubRepo, _: &str) -> Result<Vec<Release>> {
                Err(anyhow::anyhow!("API Error"))
            }
            fn api_url(&self) -> &str {
                "https://api.github.com"
            }
        }

        let installer = Installer::new(RealRuntime, FailGitHub, Client::new(), MockExtractor);

        let root_dir = tempdir().unwrap();
        let result = installer
            .get_or_fetch_meta(&repo, Some(root_dir.path()))
            .await;

        assert!(result.is_err());
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
        let installer = Installer::new(RealRuntime, mock_github, client, mock_extractor);

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
            async fn get_repo_info(&self, repo: &GitHubRepo) -> Result<RepoInfo> {
                self.get_repo_info_at(repo, self.api_url()).await
            }

            async fn get_releases(&self, repo: &GitHubRepo) -> Result<Vec<Release>> {
                self.get_releases_at(repo, self.api_url()).await
            }

            async fn get_repo_info_at(
                &self,
                _repo: &GitHubRepo,
                _api_url: &str,
            ) -> Result<RepoInfo> {
                Err(anyhow::anyhow!("Failed to get repo info"))
            }

            async fn get_releases_at(
                &self,
                _repo: &GitHubRepo,
                _api_url: &str,
            ) -> Result<Vec<Release>> {
                Ok(vec![self.release.clone()])
            }

            fn api_url(&self) -> &str {
                "https://api.github.com"
            }
        }

        let mock_github = MockGitHubFails {
            release: release.clone(),
        };
        let client = Client::new();
        let mock_extractor = MockExtractor;
        let installer = Installer::new(RealRuntime, mock_github, client, mock_extractor);

        let root_dir = tempdir().unwrap();
        let install_root = root_dir.path().to_path_buf();

        let _m = server
            .mock("GET", "/download")
            .with_status(200)
            .with_body("test")
            .create();

        // In the new flow, if get_repo_info fails and meta.json is missing,
        // it's a fatal error because we can't resolve the latest stable version.
        let result = installer.install(&repo, Some(install_root.clone())).await;

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to get repo info")
        );

        // Verify that the metadata file was not created
        let meta_file = install_root.join("owner/repo-metadata-fails/meta.json");
        assert!(!meta_file.exists());
    }

    #[tokio::test]
    async fn test_install_uses_existing_meta() {
        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo-existing-meta".to_string(),
        };

        let root_dir = tempdir().unwrap();
        let install_root = root_dir.path().to_path_buf();
        let meta_dir = install_root.join("owner/repo-existing-meta");
        fs::create_dir_all(&meta_dir).unwrap();
        let meta_path = meta_dir.join("meta.json");

        let existing_meta = Meta {
            name: "owner/repo-existing-meta".to_string(),
            api_url: "https://api.github.com".to_string(),
            repo_info_url: "https://api.github.com/repos/owner/repo-existing-meta".to_string(),
            releases_url: "https://api.github.com/repos/owner/repo-existing-meta/releases"
                .to_string(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "2023-01-01T00:00:00Z".to_string(),
            current_version: "v1.0.0".to_string(),
            releases: vec![MetaRelease {
                version: "v1.0.0".to_string(),
                title: None,
                published_at: Some("2023-01-01T00:00:00Z".to_string()),
                is_prerelease: false,
                tarball_url: "http://example.com/v1.0.0.tar.gz".to_string(),
                assets: vec![],
            }],
        };
        fs::write(&meta_path, serde_json::to_string(&existing_meta).unwrap()).unwrap();

        struct PanicGitHub;
        #[async_trait]
        impl GetReleases for PanicGitHub {
            async fn get_repo_info(&self, _: &GitHubRepo) -> Result<RepoInfo> {
                panic!("Should not be called")
            }
            async fn get_releases(&self, _: &GitHubRepo) -> Result<Vec<Release>> {
                panic!("Should not be called")
            }
            async fn get_repo_info_at(&self, _: &GitHubRepo, _: &str) -> Result<RepoInfo> {
                panic!("Should not be called")
            }
            async fn get_releases_at(&self, _: &GitHubRepo, _: &str) -> Result<Vec<Release>> {
                panic!("Should not be called")
            }
            fn api_url(&self) -> &str {
                "https://api.github.com"
            }
        }

        // We need a real server for download if we don't mock it,
        // but ensure_installed is what does the download.
        // We can mock Extractor too.
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/v1.0.0.tar.gz")
            .with_status(200)
            .create();

        // Update tarball_url in meta to point to our mock server
        let mut meta_with_mock_url = existing_meta.clone();
        meta_with_mock_url.releases[0].tarball_url = format!("{}/v1.0.0.tar.gz", server.url());
        fs::write(
            &meta_path,
            serde_json::to_string(&meta_with_mock_url).unwrap(),
        )
        .unwrap();

        let installer = Installer::new(RealRuntime, PanicGitHub, Client::new(), MockExtractor);

        installer
            .install(&repo, Some(install_root.clone()))
            .await
            .unwrap();

        let target_dir = install_root.join("owner/repo-existing-meta/v1.0.0");
        assert!(target_dir.exists());
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
            runtime: RealRuntime,
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
            runtime: RealRuntime,
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
            fn extract<R: Runtime>(
                &self,
                _runtime: &R,
                _archive_path: &Path,
                _extract_to: &Path,
            ) -> Result<()> {
                panic!("should not be called");
            }
        }

        let client = Client::new();
        ensure_installed(
            &RealRuntime,
            &target_dir,
            &repo,
            &release,
            &client,
            &FailExtractor,
        )
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
        ensure_installed(
            &RealRuntime,
            &target_dir,
            &repo,
            &release,
            &client,
            &mock_extractor,
        )
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

        let packages = find_all_packages(&RealRuntime, root).unwrap();
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
            api_url: "https://api.github.com".to_string(),
            repo_info_url: "https://api.github.com/repos/owner/repo".to_string(),
            releases_url: "https://api.github.com/repos/owner/repo/releases".to_string(),
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
        let installer = Installer::new(RealRuntime, mock_github, client, mock_extractor);

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

        let updated_meta = Meta::load(&RealRuntime, &meta_path).unwrap();
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
            api_url: server.url(),
            repo_info_url: format!("{}/repos/owner/repo", server.url()),
            releases_url: format!("{}/repos/owner/repo/releases", server.url()),
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
        let installer = Installer::new(RealRuntime, github, reqwest::Client::new(), MockExtractor);

        installer
            .update_all(Some(root.to_path_buf()))
            .await
            .unwrap();

        let updated_meta = Meta::load(&RealRuntime, &meta_path).unwrap();
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
            api_url: server.url(),
            repo_info_url: format!("{}/repos/owner/repo", server.url()),
            releases_url: format!("{}/repos/owner/repo/releases", server.url()),
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
        let installer = Installer::new(RealRuntime, github, reqwest::Client::new(), MockExtractor);

        installer
            .update_all(Some(root.to_path_buf()))
            .await
            .unwrap();

        let updated_meta = Meta::load(&RealRuntime, &meta_path).unwrap();
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

        let initial_content = r#"{
            "name": "owner/repo",
            "api_url": "https://api.github.com",
            "repo_info_url": "https://api.github.com/repos/owner/repo",
            "releases_url": "https://api.github.com/repos/owner/repo/releases",
            "current_version": "v1.0.0",
            "releases": [],
            "updated_at": "old"
        }"#;
        fs::write(&meta_path, initial_content).unwrap();

        // Mock returns 500 error to simulate failure midway
        let _m = server
            .mock("GET", "/repos/owner/repo")
            .with_status(500)
            .create();

        let github = crate::github::GitHub::new(reqwest::Client::new(), Some(server.url()));
        let installer = Installer::new(RealRuntime, github, reqwest::Client::new(), MockExtractor);

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

    #[test]
    fn test_meta_releases_sorting() {
        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
        };
        let info = RepoInfo {
            description: None,
            homepage: None,
            license: None,
            updated_at: "2023-01-01T00:00:00Z".to_string(),
        };
        let releases = vec![
            Release {
                tag_name: "v1.0.0".to_string(),
                tarball_url: "url1".to_string(),
                name: None,
                published_at: Some("2023-01-01T00:00:00Z".to_string()),
                prerelease: false,
                assets: vec![],
            },
            Release {
                tag_name: "v2.0.0".to_string(),
                tarball_url: "url2".to_string(),
                name: None,
                published_at: Some("2023-02-01T00:00:00Z".to_string()),
                prerelease: false,
                assets: vec![],
            },
            Release {
                tag_name: "v0.9.0".to_string(),
                tarball_url: "url3".to_string(),
                name: None,
                published_at: Some("2022-12-01T00:00:00Z".to_string()),
                prerelease: false,
                assets: vec![],
            },
        ];

        let meta = Meta::from(repo, info, releases, "v2.0.0", "https://api.github.com");

        // Should be sorted by published_at DESC: v2.0.0, v1.0.0, v0.9.0
        assert_eq!(meta.releases[0].version, "v2.0.0");
        assert_eq!(meta.releases[1].version, "v1.0.0");
        assert_eq!(meta.releases[2].version, "v0.9.0");
    }

    #[test]
    fn test_meta_merge_sorting() {
        let mut meta = Meta {
            name: "owner/repo".to_string(),
            api_url: "https://api.github.com".to_string(),
            repo_info_url: "https://api.github.com/repos/owner/repo".to_string(),
            releases_url: "https://api.github.com/repos/owner/repo/releases".to_string(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "2023-01-01T00:00:00Z".to_string(),
            current_version: "v1.0.0".to_string(),
            releases: vec![MetaRelease {
                version: "v1.0.0".to_string(),
                title: None,
                published_at: Some("2023-01-01T00:00:00Z".to_string()),
                is_prerelease: false,
                tarball_url: "url1".to_string(),
                assets: vec![],
            }],
        };

        let other = Meta {
            name: "owner/repo".to_string(),
            api_url: "https://api.github.com".to_string(),
            repo_info_url: "https://api.github.com/repos/owner/repo".to_string(),
            releases_url: "https://api.github.com/repos/owner/repo/releases".to_string(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "2023-02-01T00:00:00Z".to_string(),
            current_version: "v1.0.0".to_string(),
            releases: vec![MetaRelease {
                version: "v2.0.0".to_string(),
                title: None,
                published_at: Some("2023-02-01T00:00:00Z".to_string()),
                is_prerelease: false,
                tarball_url: "url2".to_string(),
                assets: vec![],
            }],
        };

        meta.merge(other);

        // Should be sorted by published_at DESC: v2.0.0, v1.0.0
        assert_eq!(meta.releases[0].version, "v2.0.0");
        assert_eq!(meta.releases[1].version, "v1.0.0");
    }

    #[test]
    fn test_meta_sorting_fallback() {
        let mut releases = vec![
            MetaRelease {
                version: "v1.0.0".to_string(),
                title: None,
                published_at: None,
                is_prerelease: false,
                tarball_url: "url1".to_string(),
                assets: vec![],
            },
            MetaRelease {
                version: "v2.0.0".to_string(),
                title: None,
                published_at: None,
                is_prerelease: false,
                tarball_url: "url2".to_string(),
                assets: vec![],
            },
            MetaRelease {
                version: "v1.5.0".to_string(),
                title: None,
                published_at: Some("2023-01-01T00:00:00Z".to_string()),
                is_prerelease: false,
                tarball_url: "url3".to_string(),
                assets: vec![],
            },
        ];

        Meta::sort_releases_internal(&mut releases);

        // Published comes first (v1.5.0), then version DESC (v2.0.0, v1.0.0)
        assert_eq!(releases[0].version, "v1.5.0");
        assert_eq!(releases[1].version, "v2.0.0");
        assert_eq!(releases[2].version, "v1.0.0");
    }

    #[tokio::test]
    async fn test_update_all_feedback() {
        let mut server = mockito::Server::new_async().await;
        let dir = tempdir().unwrap();
        let root = dir.path();
        let meta_path = root.join("owner/repo/meta.json");
        fs::create_dir_all(meta_path.parent().unwrap()).unwrap();

        let initial_meta = Meta {
            name: "owner/repo".to_string(),
            api_url: "https://api.github.com".to_string(),
            repo_info_url: "https://api.github.com/repos/owner/repo".to_string(),
            releases_url: "https://api.github.com/repos/owner/repo/releases".to_string(),
            description: None,
            homepage: None,
            license: None,
            updated_at: "old".to_string(),
            current_version: "v1.0.0".to_string(),
            releases: vec![],
        };
        fs::write(&meta_path, serde_json::to_string(&initial_meta).unwrap()).unwrap();

        let _m1 = server
            .mock("GET", "/repos/owner/repo")
            .with_status(200)
            .with_body(r#"{"updated_at": "new"}"#)
            .create();
        let _m2 = server
            .mock("GET", "/repos/owner/repo/releases?per_page=100&page=1")
            .with_status(200)
            .with_body(r#"[{"tag_name": "v2.0.0", "tarball_url": "url2", "prerelease": false, "assets": []}]"#)
            .create();

        let github = crate::github::GitHub::new(reqwest::Client::new(), Some(server.url()));
        let installer = Installer::new(RealRuntime, github, reqwest::Client::new(), MockExtractor);

        // We can't easily capture stdout in unit tests without extra crates,
        // but we can verify it doesn't crash and the logic runs.
        // The manual verification will be more important for "seeing" the output.
        installer
            .update_all(Some(root.to_path_buf()))
            .await
            .unwrap();
    }

    #[test]
    fn test_meta_serialization_with_api_urls() {
        let repo = GitHubRepo {
            owner: "owner".to_string(),
            repo: "repo".to_string(),
        };
        let info = RepoInfo {
            description: None,
            homepage: None,
            license: None,
            updated_at: "2023-01-01T00:00:00Z".to_string(),
        };
        let api_url = "https://github.custom.com/api/v3";
        let meta = Meta::from(repo, info, vec![], "v1.0.0", api_url);

        let json = serde_json::to_string(&meta).unwrap();
        let deserialized: Meta = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.api_url, api_url);
        assert_eq!(
            deserialized.repo_info_url,
            format!("{}/repos/owner/repo", api_url)
        );
        assert_eq!(
            deserialized.releases_url,
            format!("{}/repos/owner/repo/releases", api_url)
        );
    }
}
