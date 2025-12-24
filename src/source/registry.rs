//! Source registry for managing multiple package sources.
//!
//! This module provides a registry for dynamically registering and resolving
//! package sources (GitHub, GitLab, Gitee, etc.).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};

use super::{RepoId, Source, SourceKind};
use crate::package::Meta;

/// Package specification for identifying a package and its source.
#[derive(Debug, Clone)]
pub struct PackageSpec {
    /// Repository identifier (owner/repo format)
    pub repo: RepoId,
    /// Version constraint (None = latest stable)
    pub version: Option<String>,
    /// Source kind (None = use default or infer from meta)
    pub source_kind: Option<SourceKind>,
    /// Custom API URL (overrides default for the source kind)
    pub api_url: Option<String>,
}

impl PackageSpec {
    /// Create a new package spec with just a repo ID.
    pub fn new(repo: RepoId) -> Self {
        Self {
            repo,
            version: None,
            source_kind: None,
            api_url: None,
        }
    }

    /// Create a package spec with a specific version.
    pub fn with_version(repo: RepoId, version: impl Into<String>) -> Self {
        Self {
            repo,
            version: Some(version.into()),
            source_kind: None,
            api_url: None,
        }
    }

    /// Set the source kind.
    pub fn source(mut self, kind: SourceKind) -> Self {
        self.source_kind = Some(kind);
        self
    }

    /// Set a custom API URL.
    pub fn api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = Some(url.into());
        self
    }
}

/// Registry for managing multiple package sources.
///
/// The registry allows:
/// - Registering sources by kind (GitHub, GitLab, etc.)
/// - Resolving the appropriate source for a package specification
/// - Inferring source from installed package metadata
pub struct SourceRegistry {
    sources: HashMap<SourceKind, Arc<dyn Source>>,
    default_kind: SourceKind,
}

impl SourceRegistry {
    /// Create a new empty registry with GitHub as the default source kind.
    pub fn new() -> Self {
        Self {
            sources: HashMap::new(),
            default_kind: SourceKind::GitHub,
        }
    }

    /// Create a new registry with a specific default source kind.
    pub fn with_default(default_kind: SourceKind) -> Self {
        Self {
            sources: HashMap::new(),
            default_kind,
        }
    }

    /// Register a source for a specific kind.
    ///
    /// If a source is already registered for this kind, it will be replaced.
    pub fn register(&mut self, source: Arc<dyn Source>) {
        let kind = source.kind();
        self.sources.insert(kind, source);
    }

    /// Get a registered source by kind.
    pub fn get(&self, kind: SourceKind) -> Option<&Arc<dyn Source>> {
        self.sources.get(&kind)
    }

    /// Get the default source kind.
    pub fn default_kind(&self) -> SourceKind {
        self.default_kind
    }

    /// Set the default source kind.
    pub fn set_default(&mut self, kind: SourceKind) {
        self.default_kind = kind;
    }

    /// Check if a source is registered for a specific kind.
    pub fn has(&self, kind: SourceKind) -> bool {
        self.sources.contains_key(&kind)
    }

    /// Get the number of registered sources.
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Check if no sources are registered.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Resolve the appropriate source for a package specification.
    ///
    /// Resolution order:
    /// 1. If `spec.source_kind` is specified, use that source
    /// 2. Otherwise, use the default source
    ///
    /// Returns an error if the required source is not registered.
    pub fn resolve(&self, spec: &PackageSpec) -> Result<&Arc<dyn Source>> {
        let kind = spec.source_kind.unwrap_or(self.default_kind);
        self.sources
            .get(&kind)
            .with_context(|| format!("No source registered for kind: {}", kind))
    }

    /// Resolve a source from installed package metadata.
    ///
    /// This is useful for operations like update/upgrade where we need to
    /// fetch new releases from the original source.
    ///
    /// The source kind is inferred from the stored API URL in the metadata.
    pub fn resolve_from_meta(&self, meta: &Meta) -> Result<&Arc<dyn Source>> {
        let kind = Self::infer_source_kind(&meta.api_url);
        self.sources.get(&kind).with_context(|| {
            format!(
                "No source registered for kind: {} (inferred from {})",
                kind, meta.api_url
            )
        })
    }

    /// Infer source kind from an API URL.
    ///
    /// - URLs containing "github" -> GitHub
    /// - URLs containing "gitlab" -> GitLab
    /// - URLs containing "gitee" -> Gitee
    /// - Otherwise -> GitHub (default)
    fn infer_source_kind(api_url: &str) -> SourceKind {
        let url_lower = api_url.to_lowercase();
        if url_lower.contains("gitlab") {
            SourceKind::GitLab
        } else if url_lower.contains("gitee") {
            SourceKind::Gitee
        } else {
            // Default to GitHub (includes github.com and GitHub Enterprise)
            SourceKind::GitHub
        }
    }

    /// Get all registered source kinds.
    pub fn registered_kinds(&self) -> Vec<SourceKind> {
        self.sources.keys().copied().collect()
    }
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MockSource;

    fn make_mock_source(kind: SourceKind) -> Arc<dyn Source> {
        let mut mock = MockSource::new();
        mock.expect_kind().return_const(kind);
        mock.expect_api_url().return_const(match kind {
            SourceKind::GitHub => "https://api.github.com".to_string(),
            SourceKind::GitLab => "https://gitlab.com/api/v4".to_string(),
            SourceKind::Gitee => "https://gitee.com/api/v5".to_string(),
        });
        Arc::new(mock)
    }

    #[test]
    fn test_registry_new() {
        let registry = SourceRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.default_kind(), SourceKind::GitHub);
    }

    #[test]
    fn test_registry_with_default() {
        let registry = SourceRegistry::with_default(SourceKind::GitLab);
        assert_eq!(registry.default_kind(), SourceKind::GitLab);
    }

    #[test]
    fn test_registry_register_and_get() {
        let mut registry = SourceRegistry::new();
        let source = make_mock_source(SourceKind::GitHub);

        registry.register(source);

        assert!(registry.has(SourceKind::GitHub));
        assert!(!registry.has(SourceKind::GitLab));
        assert_eq!(registry.len(), 1);

        let retrieved = registry.get(SourceKind::GitHub).unwrap();
        assert_eq!(retrieved.kind(), SourceKind::GitHub);
    }

    #[test]
    fn test_registry_register_multiple() {
        let mut registry = SourceRegistry::new();
        registry.register(make_mock_source(SourceKind::GitHub));
        registry.register(make_mock_source(SourceKind::GitLab));

        assert_eq!(registry.len(), 2);
        assert!(registry.has(SourceKind::GitHub));
        assert!(registry.has(SourceKind::GitLab));
    }

    #[test]
    fn test_registry_register_replaces() {
        let mut registry = SourceRegistry::new();
        registry.register(make_mock_source(SourceKind::GitHub));
        registry.register(make_mock_source(SourceKind::GitHub)); // Replace

        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_resolve_with_explicit_kind() {
        let mut registry = SourceRegistry::new();
        registry.register(make_mock_source(SourceKind::GitHub));
        registry.register(make_mock_source(SourceKind::GitLab));

        let spec = PackageSpec::new("owner/repo".parse().unwrap()).source(SourceKind::GitLab);

        let source = registry.resolve(&spec).unwrap();
        assert_eq!(source.kind(), SourceKind::GitLab);
    }

    #[test]
    fn test_resolve_uses_default() {
        let mut registry = SourceRegistry::new();
        registry.register(make_mock_source(SourceKind::GitHub));

        let spec = PackageSpec::new("owner/repo".parse().unwrap());

        let source = registry.resolve(&spec).unwrap();
        assert_eq!(source.kind(), SourceKind::GitHub);
    }

    #[test]
    fn test_resolve_missing_source_error() {
        let registry = SourceRegistry::new(); // No sources registered

        let spec = PackageSpec::new("owner/repo".parse().unwrap());

        let result = registry.resolve(&spec);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("No source registered"));
    }

    #[test]
    fn test_resolve_from_meta_github() {
        let mut registry = SourceRegistry::new();
        registry.register(make_mock_source(SourceKind::GitHub));

        let meta = Meta {
            name: "owner/repo".into(),
            api_url: "https://api.github.com".into(),
            ..Default::default()
        };

        let source = registry.resolve_from_meta(&meta).unwrap();
        assert_eq!(source.kind(), SourceKind::GitHub);
    }

    #[test]
    fn test_resolve_from_meta_gitlab() {
        let mut registry = SourceRegistry::new();
        registry.register(make_mock_source(SourceKind::GitLab));

        let meta = Meta {
            name: "owner/repo".into(),
            api_url: "https://gitlab.com/api/v4".into(),
            ..Default::default()
        };

        let source = registry.resolve_from_meta(&meta).unwrap();
        assert_eq!(source.kind(), SourceKind::GitLab);
    }

    #[test]
    fn test_resolve_from_meta_gitee() {
        let mut registry = SourceRegistry::new();
        registry.register(make_mock_source(SourceKind::Gitee));

        let meta = Meta {
            name: "owner/repo".into(),
            api_url: "https://gitee.com/api/v5".into(),
            ..Default::default()
        };

        let source = registry.resolve_from_meta(&meta).unwrap();
        assert_eq!(source.kind(), SourceKind::Gitee);
    }

    #[test]
    fn test_resolve_from_meta_github_enterprise() {
        let mut registry = SourceRegistry::new();
        registry.register(make_mock_source(SourceKind::GitHub));

        let meta = Meta {
            name: "owner/repo".into(),
            api_url: "https://github.mycompany.com/api/v3".into(),
            ..Default::default()
        };

        // GitHub Enterprise URLs don't contain "github" in subdomain pattern
        // but should still default to GitHub
        let source = registry.resolve_from_meta(&meta).unwrap();
        assert_eq!(source.kind(), SourceKind::GitHub);
    }

    #[test]
    fn test_infer_source_kind() {
        assert_eq!(
            SourceRegistry::infer_source_kind("https://api.github.com"),
            SourceKind::GitHub
        );
        assert_eq!(
            SourceRegistry::infer_source_kind("https://gitlab.com/api/v4"),
            SourceKind::GitLab
        );
        assert_eq!(
            SourceRegistry::infer_source_kind("https://gitee.com/api/v5"),
            SourceKind::Gitee
        );
        // Unknown defaults to GitHub
        assert_eq!(
            SourceRegistry::infer_source_kind("https://unknown.com/api"),
            SourceKind::GitHub
        );
    }

    #[test]
    fn test_package_spec_builder() {
        let spec = PackageSpec::new("owner/repo".parse().unwrap())
            .source(SourceKind::GitLab)
            .api_url("https://custom.gitlab.com/api/v4");

        assert_eq!(spec.repo.owner, "owner");
        assert_eq!(spec.repo.repo, "repo");
        assert_eq!(spec.source_kind, Some(SourceKind::GitLab));
        assert_eq!(
            spec.api_url,
            Some("https://custom.gitlab.com/api/v4".into())
        );
        assert_eq!(spec.version, None);
    }

    #[test]
    fn test_package_spec_with_version() {
        let spec = PackageSpec::with_version("owner/repo".parse().unwrap(), "v1.0.0");

        assert_eq!(spec.version, Some("v1.0.0".into()));
    }

    #[test]
    fn test_registered_kinds() {
        let mut registry = SourceRegistry::new();
        registry.register(make_mock_source(SourceKind::GitHub));
        registry.register(make_mock_source(SourceKind::GitLab));

        let kinds = registry.registered_kinds();
        assert_eq!(kinds.len(), 2);
        assert!(kinds.contains(&SourceKind::GitHub));
        assert!(kinds.contains(&SourceKind::GitLab));
    }
}
