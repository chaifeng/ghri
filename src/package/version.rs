//! Version resolution for packages.
//!
//! This module provides utilities for resolving version constraints
//! and selecting appropriate releases from a list.

use crate::provider::Release;

/// Version constraint for selecting a release.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum VersionConstraint {
    /// Match exact version (e.g., "v1.2.3")
    Exact(String),
    /// Latest stable (non-prerelease) version
    #[default]
    LatestStable,
    /// Latest version including prereleases
    Latest,
}

/// Version resolver - pure functions for version resolution.
///
/// All methods are stateless and operate on slices of releases.
pub struct VersionResolver;

impl VersionResolver {
    /// Resolve a version constraint to a specific release.
    ///
    /// Returns the release that best matches the constraint, or None if no match found.
    pub fn resolve<'a>(
        releases: &'a [Release],
        constraint: &VersionConstraint,
    ) -> Option<&'a Release> {
        match constraint {
            VersionConstraint::Exact(version) => Self::find_exact(releases, version),
            VersionConstraint::LatestStable => Self::find_latest_stable(releases),
            VersionConstraint::Latest => Self::find_latest(releases),
        }
    }

    /// Find a release with exact version match.
    ///
    /// Supports matching with or without 'v' prefix (e.g., "1.0.0" matches "v1.0.0").
    pub fn find_exact<'a>(releases: &'a [Release], version: &str) -> Option<&'a Release> {
        releases
            .iter()
            .find(|r| Self::versions_match(&r.tag, version))
    }

    /// Find the latest stable (non-prerelease) release.
    ///
    /// Uses `published_at` for comparison when available, falls back to version string.
    pub fn find_latest_stable(releases: &[Release]) -> Option<&Release> {
        releases
            .iter()
            .filter(|r| !r.prerelease)
            .max_by(|a, b| Self::compare_releases(a, b))
    }

    /// Find the latest release including prereleases.
    ///
    /// Uses `published_at` for comparison when available, falls back to version string.
    pub fn find_latest(releases: &[Release]) -> Option<&Release> {
        releases.iter().max_by(|a, b| Self::compare_releases(a, b))
    }

    /// Check if there's a newer version available.
    ///
    /// Returns the newer release if available, None if current is latest.
    pub fn check_update<'a>(
        releases: &'a [Release],
        current_version: &str,
        include_prerelease: bool,
    ) -> Option<&'a Release> {
        let latest = if include_prerelease {
            Self::find_latest(releases)
        } else {
            Self::find_latest_stable(releases)
        };

        latest.filter(|r| !Self::versions_match(&r.tag, current_version))
    }

    /// Check if two version strings match.
    ///
    /// Handles the 'v' prefix flexibly (e.g., "v1.0.0" matches "1.0.0").
    pub fn versions_match(v1: &str, v2: &str) -> bool {
        let n1 = v1.strip_prefix('v').unwrap_or(v1);
        let n2 = v2.strip_prefix('v').unwrap_or(v2);
        n1 == n2
    }

    /// Compare two releases for ordering.
    ///
    /// Primary sort by `published_at` (descending), fallback to version string.
    fn compare_releases(a: &Release, b: &Release) -> std::cmp::Ordering {
        match (&a.published_at, &b.published_at) {
            (Some(at_a), Some(at_b)) => at_a.cmp(at_b),
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => a.tag.cmp(&b.tag),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_release(tag: &str, published_at: Option<&str>, prerelease: bool) -> Release {
        Release {
            tag: tag.to_string(),
            published_at: published_at.map(String::from),
            prerelease,
            ..Default::default()
        }
    }

    #[test]
    fn test_versions_match_exact() {
        assert!(VersionResolver::versions_match("v1.0.0", "v1.0.0"));
        assert!(VersionResolver::versions_match("1.0.0", "1.0.0"));
    }

    #[test]
    fn test_versions_match_with_v_prefix() {
        assert!(VersionResolver::versions_match("v1.0.0", "1.0.0"));
        assert!(VersionResolver::versions_match("1.0.0", "v1.0.0"));
    }

    #[test]
    fn test_versions_match_different() {
        assert!(!VersionResolver::versions_match("v1.0.0", "v2.0.0"));
        assert!(!VersionResolver::versions_match("v1.0.0", "v1.0.1"));
    }

    #[test]
    fn test_find_exact() {
        let releases = vec![
            make_release("v1.0.0", Some("2024-01-01T00:00:00Z"), false),
            make_release("v2.0.0", Some("2024-02-01T00:00:00Z"), false),
        ];

        let result = VersionResolver::find_exact(&releases, "v1.0.0");
        assert!(result.is_some());
        assert_eq!(result.unwrap().tag, "v1.0.0");

        // Match without v prefix
        let result = VersionResolver::find_exact(&releases, "2.0.0");
        assert!(result.is_some());
        assert_eq!(result.unwrap().tag, "v2.0.0");

        // No match
        let result = VersionResolver::find_exact(&releases, "v3.0.0");
        assert!(result.is_none());
    }

    #[test]
    fn test_find_latest_stable() {
        let releases = vec![
            make_release("v1.0.0", Some("2024-01-01T00:00:00Z"), false),
            make_release("v2.0.0", Some("2024-02-01T00:00:00Z"), false),
            make_release("v3.0.0-rc1", Some("2024-03-01T00:00:00Z"), true), // prerelease
        ];

        let result = VersionResolver::find_latest_stable(&releases);
        assert!(result.is_some());
        assert_eq!(result.unwrap().tag, "v2.0.0"); // Should skip prerelease
    }

    #[test]
    fn test_find_latest_stable_only_prereleases() {
        let releases = vec![
            make_release("v1.0.0-alpha", Some("2024-01-01T00:00:00Z"), true),
            make_release("v1.0.0-beta", Some("2024-02-01T00:00:00Z"), true),
        ];

        let result = VersionResolver::find_latest_stable(&releases);
        assert!(result.is_none()); // No stable releases
    }

    #[test]
    fn test_find_latest_includes_prerelease() {
        let releases = vec![
            make_release("v1.0.0", Some("2024-01-01T00:00:00Z"), false),
            make_release("v2.0.0", Some("2024-02-01T00:00:00Z"), false),
            make_release("v3.0.0-rc1", Some("2024-03-01T00:00:00Z"), true), // prerelease
        ];

        let result = VersionResolver::find_latest(&releases);
        assert!(result.is_some());
        assert_eq!(result.unwrap().tag, "v3.0.0-rc1"); // Should include prerelease
    }

    #[test]
    fn test_find_latest_empty() {
        let releases: Vec<Release> = vec![];
        assert!(VersionResolver::find_latest(&releases).is_none());
        assert!(VersionResolver::find_latest_stable(&releases).is_none());
    }

    #[test]
    fn test_resolve_exact() {
        let releases = vec![
            make_release("v1.0.0", Some("2024-01-01T00:00:00Z"), false),
            make_release("v2.0.0", Some("2024-02-01T00:00:00Z"), false),
        ];

        let constraint = VersionConstraint::Exact("v1.0.0".into());
        let result = VersionResolver::resolve(&releases, &constraint);
        assert!(result.is_some());
        assert_eq!(result.unwrap().tag, "v1.0.0");
    }

    #[test]
    fn test_resolve_latest_stable() {
        let releases = vec![
            make_release("v1.0.0", Some("2024-01-01T00:00:00Z"), false),
            make_release("v2.0.0-rc1", Some("2024-02-01T00:00:00Z"), true),
        ];

        let constraint = VersionConstraint::LatestStable;
        let result = VersionResolver::resolve(&releases, &constraint);
        assert!(result.is_some());
        assert_eq!(result.unwrap().tag, "v1.0.0");
    }

    #[test]
    fn test_resolve_latest() {
        let releases = vec![
            make_release("v1.0.0", Some("2024-01-01T00:00:00Z"), false),
            make_release("v2.0.0-rc1", Some("2024-02-01T00:00:00Z"), true),
        ];

        let constraint = VersionConstraint::Latest;
        let result = VersionResolver::resolve(&releases, &constraint);
        assert!(result.is_some());
        assert_eq!(result.unwrap().tag, "v2.0.0-rc1");
    }

    #[test]
    fn test_check_update_available() {
        let releases = vec![
            make_release("v1.0.0", Some("2024-01-01T00:00:00Z"), false),
            make_release("v2.0.0", Some("2024-02-01T00:00:00Z"), false),
        ];

        let update = VersionResolver::check_update(&releases, "v1.0.0", false);
        assert!(update.is_some());
        assert_eq!(update.unwrap().tag, "v2.0.0");
    }

    #[test]
    fn test_check_update_already_latest() {
        let releases = vec![
            make_release("v1.0.0", Some("2024-01-01T00:00:00Z"), false),
            make_release("v2.0.0", Some("2024-02-01T00:00:00Z"), false),
        ];

        let update = VersionResolver::check_update(&releases, "v2.0.0", false);
        assert!(update.is_none()); // Already on latest
    }

    #[test]
    fn test_check_update_with_prerelease() {
        let releases = vec![
            make_release("v1.0.0", Some("2024-01-01T00:00:00Z"), false),
            make_release("v2.0.0-rc1", Some("2024-02-01T00:00:00Z"), true),
        ];

        // Without prerelease - no update
        let update = VersionResolver::check_update(&releases, "v1.0.0", false);
        assert!(update.is_none()); // v2.0.0-rc1 is prerelease

        // With prerelease - has update
        let update = VersionResolver::check_update(&releases, "v1.0.0", true);
        assert!(update.is_some());
        assert_eq!(update.unwrap().tag, "v2.0.0-rc1");
    }

    #[test]
    fn test_check_update_version_match_with_v_prefix() {
        let releases = vec![make_release("v1.0.0", Some("2024-01-01T00:00:00Z"), false)];

        // "1.0.0" should match "v1.0.0"
        let update = VersionResolver::check_update(&releases, "1.0.0", false);
        assert!(update.is_none()); // Already on latest (matches with v prefix)
    }

    #[test]
    fn test_compare_releases_by_published_at() {
        let older = make_release("v1.0.0", Some("2024-01-01T00:00:00Z"), false);
        let newer = make_release("v0.9.0", Some("2024-02-01T00:00:00Z"), false);

        // v0.9.0 is "newer" by published_at despite lower version number
        let releases = vec![older, newer];
        let latest = VersionResolver::find_latest(&releases);
        assert_eq!(latest.unwrap().tag, "v0.9.0");
    }

    #[test]
    fn test_compare_releases_fallback_to_version() {
        let v1 = make_release("v1.0.0", None, false);
        let v2 = make_release("v2.0.0", None, false);

        // Without published_at, should fall back to version string comparison
        let releases = vec![v1, v2];
        let latest = VersionResolver::find_latest(&releases);
        assert_eq!(latest.unwrap().tag, "v2.0.0");
    }
}
