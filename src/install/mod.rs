use crate::{
    archive::extract_archive,
    download::download_file,
    github::{get_latest_release, GitHubRepo},
};
use anyhow::{Context, Result};
use reqwest::Client;
use std::fs;

const GITHUB_API_URL: &str = "https://api.github.com";

pub async fn install(repo_str: &str) -> Result<()> {
    let repo = repo_str.parse::<GitHubRepo>()?;

    let client = Client::builder().user_agent("ghri-cli").build()?;

    let release = get_latest_release(&repo, &client, GITHUB_API_URL).await?;
    println!("Found latest version: {}", release.tag_name);

    let home_dir = dirs::home_dir().context("Could not find home directory")?;
    let target_dir = home_dir
        .join(".ghri")
        .join(&repo.owner)
        .join(&repo.repo)
        .join(&release.tag_name);

    if target_dir.exists() {
        println!(
            "Directory {:?} already exists. Skipping download and extraction.",
            target_dir
        );
        return Ok(());
    }

    println!("Creating target directory: {:?}", target_dir);
    fs::create_dir_all(&target_dir)
        .with_context(|| format!("Failed to create target directory at {:?}", target_dir))?;

    let temp_dir = std::env::temp_dir();
    let temp_file_path = temp_dir.join(format!("{}-{}.tar.gz", repo.repo, release.tag_name));

    download_file(&release.tarball_url, &temp_file_path, &client).await?;
    extract_archive(&temp_file_path, &target_dir)?;

    fs::remove_file(&temp_file_path)
        .with_context(|| format!("Failed to clean up temporary file: {:?}", temp_file_path))?;

    println!(
        "\nSuccessfully installed {} version {} to {:?}",
        repo_str, release.tag_name, target_dir
    );

    Ok(())
}
