mod client;
mod repo;
mod types;

pub use client::{GetReleases, GitHub};
pub use repo::{GitHubRepo, LinkSpec, RepoSpec};
pub use types::{License, Release, ReleaseAsset, RepoInfo};

#[cfg(test)]
pub use client::MockGetReleases;
