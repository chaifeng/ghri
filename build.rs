use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");

    let cargo_version = env!("CARGO_PKG_VERSION");

    let output = Command::new("git")
        .args(&["describe", "--tags", "--always", "--dirty"])
        .output();
    let now = SystemTime::now();
    let secs = now
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs();
    let git_desc = match output {
        Ok(o) if o.status.success() => {
            let git_output = String::from_utf8(o.stdout)
                .unwrap_or_default()
                .trim()
                .to_string();
            let git_version = git_output
                .strip_prefix(cargo_version)
                .unwrap_or(&git_output)
                .to_string();
            match git_version {
                git_version if git_version.ends_with("-dirty") => {
                    format!("-{}-{}", git_version, secs)
                }
                git_version if git_version.is_empty() => "".to_string(),
                _ => format!("-{}", git_version),
            }
        }
        _ => format!("-unknown-{}", secs),
    };

    println!("cargo:rustc-env=GHRI_VERSION={}{}", cargo_version, git_desc);
}
