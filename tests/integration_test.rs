use assert_cmd::Command;
use assert_cmd::cargo;
use flate2::Compression;
use flate2::write::GzEncoder;
use mockito::Server;
use std::io::prelude::*;
use tar::Builder;
use tempfile::tempdir;

fn create_tar_gz(files: &[(&str, &str)]) -> Vec<u8> {
    let mut tar_builder = Builder::new(Vec::new());
    for (name, content) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_path(name).unwrap();
        header.set_cksum();
        tar_builder.append(&header, content.as_bytes()).unwrap();
    }
    let tar = tar_builder.into_inner().unwrap();

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn test_end_to_end_install() {
    let mut server = Server::new();
    let url = server.url();

    let _mock_latest = server
        .mock("GET", "/repos/owner/repo/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
                "tag_name": "v1.0.0",
                "tarball_url": "{}/download/v1.0.0.tar.gz",
                "prerelease": false,
                "assets": []
            }}"#,
            url
        ))
        .create();

    let _mock_releases = server
        .mock("GET", "/repos/owner/repo/releases?per_page=100&page=1")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"[{{
                "tag_name": "v1.0.0",
                "tarball_url": "{}/download/v1.0.0.tar.gz",
                "prerelease": false,
                "assets": []
            }}]"#,
            url
        ))
        .create();

    let _mock_repo = server
        .mock("GET", "/repos/owner/repo")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
                "description": "A test repo",
                "homepage": "https://example.com",
                "license": { "key": "mit", "name": "MIT License" },
                "updated_at": "2023-01-01T00:00:00Z"
            }"#,
        )
        .create();

    let tar_gz_bytes = create_tar_gz(&[("test.txt", "hello world")]);
    let _mock_download = server
        .mock("GET", "/download/v1.0.0.tar.gz")
        .with_status(200)
        .with_body(&tar_gz_bytes)
        .create();

    let root_dir = tempdir().unwrap();
    let install_root = root_dir.path();

    let mut cmd = Command::new(cargo::cargo_bin!("ghri"));
    cmd.arg("install")
        .arg("owner/repo")
        .arg("--root")
        .arg(install_root)
        .env("GITHUB_API_URL", &url);

    cmd.assert().success();

    let target_dir = install_root.join("owner/repo/v1.0.0");
    assert!(target_dir.exists());

    let current_link = install_root.join("owner/repo/current");
    assert!(current_link.is_symlink());
    assert_eq!(
        std::fs::read_link(&current_link).unwrap(),
        std::path::Path::new("v1.0.0")
    );

    let meta_file = install_root.join("owner/repo/meta.json");
    assert!(meta_file.exists());
    let meta_content = std::fs::read_to_string(meta_file).unwrap();
    assert!(meta_content.contains("v1.0.0"));
    assert!(meta_content.contains("owner/repo"));
}
