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

fn create_tar_gz_with_executable(files: &[(&str, &str, u32)]) -> Vec<u8> {
    let mut tar_builder = Builder::new(Vec::new());
    for (name, content, mode) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_path(name).unwrap();
        header.set_mode(*mode);
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
        .arg("--api-url")
        .arg(&url);

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

    // Test list command shows the installed package
    let mut list_cmd = Command::new(cargo::cargo_bin!("ghri"));
    list_cmd
        .arg("list")
        .arg("--root")
        .arg(install_root);

    list_cmd
        .assert()
        .success()
        .stdout(predicates::str::contains("owner/repo"))
        .stdout(predicates::str::contains("v1.0.0"));
}

#[test]
fn test_link_single_file_to_path() {
    // Test linking when version directory has a single file
    let mut server = Server::new();
    let url = server.url();

    let _mock_releases = server
        .mock("GET", "/repos/test/tool/releases?per_page=100&page=1")
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
        .mock("GET", "/repos/test/tool")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"description": null, "homepage": null, "license": null, "updated_at": "2023-01-01T00:00:00Z"}"#)
        .create();

    // Create archive with single executable file
    let tar_gz_bytes = create_tar_gz_with_executable(&[("test-tool-v1/tool", "#!/bin/bash\necho hello", 0o755)]);
    let _mock_download = server
        .mock("GET", "/download/v1.0.0.tar.gz")
        .with_status(200)
        .with_body(&tar_gz_bytes)
        .create();

    let root_dir = tempdir().unwrap();
    let install_root = root_dir.path();
    let link_dir = tempdir().unwrap();

    // Install the package first
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("install")
        .arg("test/tool")
        .arg("--root")
        .arg(install_root)
        .arg("--api-url")
        .arg(&url)
        .assert()
        .success();

    // Link to a specific file path
    let link_path = link_dir.path().join("my-tool");
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("link")
        .arg("test/tool")
        .arg(&link_path)
        .arg("--root")
        .arg(install_root)
        .assert()
        .success();

    // Verify symlink was created
    assert!(link_path.is_symlink());

    // Verify symlink points to the single file (not the directory)
    let link_target = std::fs::read_link(&link_path).unwrap();
    assert!(link_target.to_string_lossy().contains("tool"));

    // Verify meta.json has linked_to field
    let meta_content = std::fs::read_to_string(install_root.join("test/tool/meta.json")).unwrap();
    assert!(meta_content.contains("linked_to"));
    assert!(meta_content.contains("my-tool"));
}

#[test]
fn test_link_to_directory() {
    // Test linking when dest is an existing directory
    let mut server = Server::new();
    let url = server.url();

    let _mock_releases = server
        .mock("GET", "/repos/org/cli/releases?per_page=100&page=1")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"[{{
                "tag_name": "v2.0.0",
                "tarball_url": "{}/download/v2.0.0.tar.gz",
                "prerelease": false,
                "assets": []
            }}]"#,
            url
        ))
        .create();

    let _mock_repo = server
        .mock("GET", "/repos/org/cli")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"description": null, "homepage": null, "license": null, "updated_at": "2023-01-01T00:00:00Z"}"#)
        .create();

    let tar_gz_bytes = create_tar_gz_with_executable(&[("cli-v2/cli", "#!/bin/bash\necho cli", 0o755)]);
    let _mock_download = server
        .mock("GET", "/download/v2.0.0.tar.gz")
        .with_status(200)
        .with_body(&tar_gz_bytes)
        .create();

    let root_dir = tempdir().unwrap();
    let install_root = root_dir.path();
    let bin_dir = tempdir().unwrap();

    // Install the package
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("install")
        .arg("org/cli")
        .arg("--root")
        .arg(install_root)
        .arg("--api-url")
        .arg(&url)
        .assert()
        .success();

    // Link to a directory - should create symlink inside with repo name
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("link")
        .arg("org/cli")
        .arg(bin_dir.path())
        .arg("--root")
        .arg(install_root)
        .assert()
        .success();

    // Verify symlink was created inside the directory with repo name
    let expected_link = bin_dir.path().join("cli");
    assert!(expected_link.is_symlink(), "Expected symlink at {:?}", expected_link);

    // Verify meta.json has the full path
    let meta_content = std::fs::read_to_string(install_root.join("org/cli/meta.json")).unwrap();
    assert!(meta_content.contains("linked_to"));
    assert!(meta_content.contains("/cli\"")); // Full path ends with /cli"
}

#[test]
fn test_link_update_on_reinstall() {
    // Test that link is updated when a new version is installed
    let mut server = Server::new();
    let url = server.url();

    // First version
    let _mock_releases_v1 = server
        .mock("GET", "/repos/dev/app/releases?per_page=100&page=1")
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
        .expect(1)
        .create();

    let _mock_repo = server
        .mock("GET", "/repos/dev/app")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"description": null, "homepage": null, "license": null, "updated_at": "2023-01-01T00:00:00Z"}"#)
        .create();

    let tar_gz_v1 = create_tar_gz_with_executable(&[("app-v1/app", "v1", 0o755)]);
    let _mock_download_v1 = server
        .mock("GET", "/download/v1.0.0.tar.gz")
        .with_status(200)
        .with_body(&tar_gz_v1)
        .create();

    let root_dir = tempdir().unwrap();
    let install_root = root_dir.path();
    let link_dir = tempdir().unwrap();
    let link_path = link_dir.path().join("app");

    // Install v1
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("install")
        .arg("dev/app")
        .arg("--root")
        .arg(install_root)
        .arg("--api-url")
        .arg(&url)
        .assert()
        .success();

    // Create link
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("link")
        .arg("dev/app")
        .arg(&link_path)
        .arg("--root")
        .arg(install_root)
        .assert()
        .success();

    // Verify v1 link
    assert!(link_path.is_symlink());
    let v1_target = std::fs::read_link(&link_path).unwrap();
    assert!(v1_target.to_string_lossy().contains("v1.0.0"));

    // Now install v2
    let _mock_releases_v2 = server
        .mock("GET", "/repos/dev/app/releases?per_page=100&page=1")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"[{{
                "tag_name": "v2.0.0",
                "tarball_url": "{}/download/v2.0.0.tar.gz",
                "prerelease": false,
                "assets": []
            }}, {{
                "tag_name": "v1.0.0",
                "tarball_url": "{}/download/v1.0.0.tar.gz",
                "prerelease": false,
                "assets": []
            }}]"#,
            url, url
        ))
        .create();

    let tar_gz_v2 = create_tar_gz_with_executable(&[("app-v2/app", "v2", 0o755)]);
    let _mock_download_v2 = server
        .mock("GET", "/download/v2.0.0.tar.gz")
        .with_status(200)
        .with_body(&tar_gz_v2)
        .create();

    // First run update to get v2 into local meta
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("update")
        .arg("--root")
        .arg(install_root)
        .assert()
        .success();

    // Install v2 (should automatically update the link)
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("install")
        .arg("dev/app@v2.0.0")
        .arg("--root")
        .arg(install_root)
        .arg("--api-url")
        .arg(&url)
        .assert()
        .success();

    // Verify link now points to v2
    assert!(link_path.is_symlink());
    let v2_target = std::fs::read_link(&link_path).unwrap();
    assert!(v2_target.to_string_lossy().contains("v2.0.0"), "Expected v2.0.0 in {:?}", v2_target);
}

#[test]
fn test_link_update_existing_symlink() {
    // Test updating an existing symlink that points to a different version
    let mut server = Server::new();
    let url = server.url();

    let _mock_releases = server
        .mock("GET", "/repos/my/pkg/releases?per_page=100&page=1")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"[{{
                "tag_name": "v1.0.0",
                "tarball_url": "{}/download/v1.0.0.tar.gz",
                "prerelease": false,
                "assets": []
            }}, {{
                "tag_name": "v0.9.0",
                "tarball_url": "{}/download/v0.9.0.tar.gz",
                "prerelease": false,
                "assets": []
            }}]"#,
            url, url
        ))
        .create();

    let _mock_repo = server
        .mock("GET", "/repos/my/pkg")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"description": null, "homepage": null, "license": null, "updated_at": "2023-01-01T00:00:00Z"}"#)
        .create();

    let tar_gz_v09 = create_tar_gz_with_executable(&[("pkg-v09/pkg", "v0.9", 0o755)]);
    let _mock_download_v09 = server
        .mock("GET", "/download/v0.9.0.tar.gz")
        .with_status(200)
        .with_body(&tar_gz_v09)
        .create();

    let tar_gz_v1 = create_tar_gz_with_executable(&[("pkg-v1/pkg", "v1.0", 0o755)]);
    let _mock_download_v1 = server
        .mock("GET", "/download/v1.0.0.tar.gz")
        .with_status(200)
        .with_body(&tar_gz_v1)
        .create();

    let root_dir = tempdir().unwrap();
    let install_root = root_dir.path();
    let link_dir = tempdir().unwrap();
    let link_path = link_dir.path().join("pkg");

    // Install v0.9.0 first
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("install")
        .arg("my/pkg@v0.9.0")
        .arg("--root")
        .arg(install_root)
        .arg("--api-url")
        .arg(&url)
        .assert()
        .success();

    // Create link to v0.9.0
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("link")
        .arg("my/pkg")
        .arg(&link_path)
        .arg("--root")
        .arg(install_root)
        .assert()
        .success();

    let v09_target = std::fs::read_link(&link_path).unwrap();
    assert!(v09_target.to_string_lossy().contains("v0.9.0"));

    // Install v1.0.0 (this changes current version)
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("install")
        .arg("my/pkg@v1.0.0")
        .arg("--root")
        .arg(install_root)
        .arg("--api-url")
        .arg(&url)
        .assert()
        .success();

    // Link again - should update existing symlink
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("link")
        .arg("my/pkg")
        .arg(&link_path)
        .arg("--root")
        .arg(install_root)
        .assert()
        .success();

    // Verify link now points to v1.0.0
    let v1_target = std::fs::read_link(&link_path).unwrap();
    assert!(v1_target.to_string_lossy().contains("v1.0.0"), "Expected v1.0.0 in {:?}", v1_target);
}

#[test]
fn test_link_fails_for_uninstalled_package() {
    let root_dir = tempdir().unwrap();
    let install_root = root_dir.path();
    let link_dir = tempdir().unwrap();

    Command::new(cargo::cargo_bin!("ghri"))
        .arg("link")
        .arg("nonexistent/package")
        .arg(link_dir.path().join("link"))
        .arg("--root")
        .arg(install_root)
        .assert()
        .failure()
        .stderr(predicates::str::contains("not installed"));
}

#[test]
fn test_link_fails_for_existing_non_symlink() {
    let mut server = Server::new();
    let url = server.url();

    let _mock_releases = server
        .mock("GET", "/repos/test/blocked/releases?per_page=100&page=1")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"[{{"tag_name": "v1.0.0", "tarball_url": "{}/download/v1.0.0.tar.gz", "prerelease": false, "assets": []}}]"#,
            url
        ))
        .create();

    let _mock_repo = server
        .mock("GET", "/repos/test/blocked")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"description": null, "homepage": null, "license": null, "updated_at": "2023-01-01T00:00:00Z"}"#)
        .create();

    let tar_gz = create_tar_gz(&[("blocked-v1/blocked", "content")]);
    let _mock_download = server
        .mock("GET", "/download/v1.0.0.tar.gz")
        .with_status(200)
        .with_body(&tar_gz)
        .create();

    let root_dir = tempdir().unwrap();
    let install_root = root_dir.path();
    let link_dir = tempdir().unwrap();
    let blocking_file = link_dir.path().join("blocked");

    // Create a regular file at the link destination
    std::fs::write(&blocking_file, "I'm blocking").unwrap();

    // Install package
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("install")
        .arg("test/blocked")
        .arg("--root")
        .arg(install_root)
        .arg("--api-url")
        .arg(&url)
        .assert()
        .success();

    // Try to link - should fail because destination is a regular file
    Command::new(cargo::cargo_bin!("ghri"))
        .arg("link")
        .arg("test/blocked")
        .arg(&blocking_file)
        .arg("--root")
        .arg(install_root)
        .assert()
        .failure()
        .stderr(predicates::str::contains("not a symlink"));
}
