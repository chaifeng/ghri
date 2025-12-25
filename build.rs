use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-env-changed=GHRI_ROOT");

    // Declare the custom cfg for check-cfg lint
    println!("cargo::rustc-check-cfg=cfg(ghri_root_set)");

    // Set cfg flag if GHRI_ROOT environment variable is set
    if std::env::var("GHRI_ROOT").is_ok() {
        println!("cargo:rustc-cfg=ghri_root_set");
    }

    let output = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output();

    let version = match output {
        Ok(o) if o.status.success() => {
            let git_output = String::from_utf8(o.stdout)
                .unwrap_or_default()
                .trim()
                .to_string();

            // Strip 'v' prefix if present (e.g., "v1.0.0" -> "1.0.0")
            let version = git_output.strip_prefix('v').unwrap_or(&git_output);

            if version.ends_with("-dirty") || version.is_empty() {
                // Dirty working tree or no output: append timestamp
                format!("{}-{}", version, timestamp())
            } else {
                version.to_string()
            }
        }
        _ => {
            // Git command failed: use timestamp as version
            format!("0.0.0-unknown-{}", timestamp())
        }
    };

    println!("cargo:rustc-env=GHRI_VERSION={}", version);
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}
