use crate::{
    archive::extract_archive,
    download::download_file,
    github::{GitHubRepo, Release, get_latest_release},
};
use anyhow::{Context, Result, bail};
use log::{debug, info};
use reqwest::Client;
use std::fs;
use std::path::{Path, PathBuf};

const GITHUB_API_URL: &str = "https://api.github.com";

pub async fn install(repo_str: &str) -> Result<()> {
    let repo = repo_str.parse::<GitHubRepo>()?;
    let client = Client::builder().user_agent("ghri-cli").build()?;

    let release = get_latest_release(&repo, &client, GITHUB_API_URL).await?;
    info!("Found latest version: {}", release.tag_name);

    let target_dir = get_target_dir(&repo, &release)?;

    ensure_installed(&target_dir, &repo, &release, &client).await?;
    update_current_symlink(&target_dir, &release.tag_name)?;

    println!(
        "installed {} {} {}",
        repo_str,
        release.tag_name,
        target_dir.display()
    );

    Ok(())
}

fn get_target_dir(repo: &GitHubRepo, release: &Release) -> Result<PathBuf> {
    let home_dir = dirs::home_dir().context("Could not find home directory")?;
    Ok(home_dir
        .join(".ghri")
        .join(&repo.owner)
        .join(&repo.repo)
        .join(&release.tag_name))
}

async fn ensure_installed(
    target_dir: &Path,
    repo: &GitHubRepo,
    release: &Release,
    client: &Client,
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
    extract_archive(&temp_file_path, target_dir)?;

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
        };

        // This test assumes HOME is set or that dirs::home_dir retrieves something.
        // We mainly check the structure relative to the home dir.
        let target_dir = get_target_dir(&repo, &release).unwrap();
        // Since we can't easily mock dirs::home_dir without external crates or env var tricks safely
        // across all OSes, we just assert the suffix.
        assert!(target_dir.ends_with(".ghri/owner/repo/v1.0.0"));
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

        // Point to v1 initially
        #[cfg(unix)]
        std::os::unix::fs::symlink("v1.0.0", base_path.join("current")).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir("v1.0.0", base_path.join("current")).unwrap();

        // Update to v2
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
        // Run again
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

        // Create a regular file named 'current'
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
}
