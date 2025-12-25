//! Provider registry for managing multiple package providers.
//!
//! This module provides a registry for dynamically registering and resolving
//! package providers (GitHub, GitLab, Gitee, etc.).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};

use super::{Provider, ProviderKind, RepoId};
use crate::package::Meta;

/// Package specification for identifying a package and its provider.
#[derive(Debug, Clone)]
pub struct PackageSpec {
    /// Repository identifier (owner/repo format)
    pub repo: RepoId,
    /// Version constraint (None = latest stable)
    pub version: Option<String>,
    /// Provider kind (None = use default or infer from meta)
    pub provider_kind: Option<ProviderKind>,
    /// Custom API URL (overrides default for the provider kind)
    pub api_url: Option<String>,
}

impl PackageSpec {
    /// Create a new package spec with just a repo ID.
    pub fn new(repo: RepoId) -> Self {
        Self {
            repo,
            version: None,
            provider_kind: None,
            api_url: None,
        }
    }

    /// Create a package spec with a specific version.
    pub fn with_version(repo: RepoId, version: impl Into<String>) -> Self {
        Self {
            repo,
            version: Some(version.into()),
            provider_kind: None,
            api_url: None,
        }
    }

    /// Set the provider kind.
    pub fn provider(mut self, kind: ProviderKind) -> Self {
        self.provider_kind = Some(kind);
        self
    }

    /// Set a custom API URL.
    pub fn api_url(mut self, url: impl Into<String>) -> Self {
        self.api_url = Some(url.into());
        self
    }
}

/// Registry for managing multiple package providers.
///
/// The registry allows:
/// - Registering providers by kind (GitHub, GitLab, etc.)
/// - Resolving the appropriate provider for a package specification
/// - Inferring provider from installed package metadata
pub struct ProviderRegistry {
    providers: HashMap<ProviderKind, Arc<dyn Provider>>,
    default_kind: ProviderKind,
}

impl ProviderRegistry {
    /// Create a new empty registry with GitHub as the default provider kind.
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            default_kind: ProviderKind::GitHub,
        }
    }

    /// Create a new registry with a specific default provider kind.
    pub fn with_default(default_kind: ProviderKind) -> Self {
        Self {
            providers: HashMap::new(),
            default_kind,
        }
    }

    /// Register a provider for a specific kind.
    ///
    /// If a provider is already registered for this kind, it will be replaced.
    pub fn register(&mut self, provider: Arc<dyn Provider>) {
        let kind = provider.kind();
        self.providers.insert(kind, provider);
    }

    /// Get a registered provider by kind.
    pub fn get(&self, kind: ProviderKind) -> Option<&Arc<dyn Provider>> {
        self.providers.get(&kind)
    }

    /// Get the default provider kind.
    pub fn default_kind(&self) -> ProviderKind {
        self.default_kind
    }

    /// Set the default provider kind.
    pub fn set_default(&mut self, kind: ProviderKind) {
        self.default_kind = kind;
    }

    /// Check if a provider is registered for a specific kind.
    pub fn has(&self, kind: ProviderKind) -> bool {
        self.providers.contains_key(&kind)
    }

    /// Get the number of registered providers.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Check if no providers are registered.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Resolve the appropriate provider for a package specification.
    ///
    /// Resolution order:
    /// 1. If `spec.provider_kind` is specified, use that provider
    /// 2. Otherwise, use the default provider
    ///
    /// Returns an error if the required provider is not registered.
    pub fn resolve(&self, spec: &PackageSpec) -> Result<&Arc<dyn Provider>> {
        let kind = spec.provider_kind.unwrap_or(self.default_kind);
        self.providers
            .get(&kind)
            .with_context(|| format!("No provider registered for kind: {}", kind))
    }

    /// Resolve a provider from installed package metadata.
    ///
    /// This is useful for operations like update/upgrade where we need to
    /// fetch new releases from the original provider.
    ///
    /// The provider kind is inferred from the stored API URL in the metadata.
    pub fn resolve_from_meta(&self, meta: &Meta) -> Result<&Arc<dyn Provider>> {
        let kind = Self::infer_provider_kind(&meta.api_url);
        self.providers.get(&kind).with_context(|| {
            format!(
                "No provider registered for kind: {} (inferred from {})",
                kind, meta.api_url
            )
        })
    }

    /// Infer provider kind from an API URL.
    ///
    /// - URLs containing "github" -> GitHub
    /// - URLs containing "gitlab" -> GitLab
    /// - URLs containing "gitee" -> Gitee
    /// - Otherwise -> GitHub (default)
    fn infer_provider_kind(api_url: &str) -> ProviderKind {
        let url_lower = api_url.to_lowercase();
        if url_lower.contains("gitlab") {
            ProviderKind::GitLab
        } else if url_lower.contains("gitee") {
            ProviderKind::Gitee
        } else {
            // Default to GitHub (includes github.com and GitHub Enterprise)
            ProviderKind::GitHub
        }
    }

    /// Get all registered provider kinds.
    pub fn registered_kinds(&self) -> Vec<ProviderKind> {
        self.providers.keys().copied().collect()
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::MockProvider;

    fn make_mock_provider(kind: ProviderKind) -> Arc<dyn Provider> {
        let mut mock = MockProvider::new();
        mock.expect_kind().return_const(kind);
        mock.expect_api_url().return_const(match kind {
            ProviderKind::GitHub => "https://api.github.com".to_string(),
            ProviderKind::GitLab => "https://gitlab.com/api/v4".to_string(),
            ProviderKind::Gitee => "https://gitee.com/api/v5".to_string(),
        });
        Arc::new(mock)
    }

    #[test]
    fn test_registry_new() {
        let registry = ProviderRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.default_kind(), ProviderKind::GitHub);
    }

    #[test]
    fn test_registry_with_default() {
        let registry = ProviderRegistry::with_default(ProviderKind::GitLab);
        assert_eq!(registry.default_kind(), ProviderKind::GitLab);
    }

    #[test]
    fn test_registry_register_and_get() {
        let mut registry = ProviderRegistry::new();
        let provider = make_mock_provider(ProviderKind::GitHub);

        registry.register(provider);

        assert!(registry.has(ProviderKind::GitHub));
        assert!(!registry.has(ProviderKind::GitLab));
        assert_eq!(registry.len(), 1);

        let retrieved = registry.get(ProviderKind::GitHub).unwrap();
        assert_eq!(retrieved.kind(), ProviderKind::GitHub);
    }

    #[test]
    fn test_registry_register_multiple() {
        let mut registry = ProviderRegistry::new();
        registry.register(make_mock_provider(ProviderKind::GitHub));
        registry.register(make_mock_provider(ProviderKind::GitLab));

        assert_eq!(registry.len(), 2);
        assert!(registry.has(ProviderKind::GitHub));
        assert!(registry.has(ProviderKind::GitLab));
    }

    #[test]
    fn test_registry_register_replaces() {
        let mut registry = ProviderRegistry::new();
        registry.register(make_mock_provider(ProviderKind::GitHub));
        registry.register(make_mock_provider(ProviderKind::GitHub)); // Replace

        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_resolve_with_explicit_kind() {
        let mut registry = ProviderRegistry::new();
        registry.register(make_mock_provider(ProviderKind::GitHub));
        registry.register(make_mock_provider(ProviderKind::GitLab));

        let spec = PackageSpec::new("owner/repo".parse().unwrap()).provider(ProviderKind::GitLab);

        let provider = registry.resolve(&spec).unwrap();
        assert_eq!(provider.kind(), ProviderKind::GitLab);
    }

    #[test]
    fn test_resolve_uses_default() {
        let mut registry = ProviderRegistry::new();
        registry.register(make_mock_provider(ProviderKind::GitHub));

        let spec = PackageSpec::new("owner/repo".parse().unwrap());

        let provider = registry.resolve(&spec).unwrap();
        assert_eq!(provider.kind(), ProviderKind::GitHub);
    }

    #[test]
    fn test_resolve_missing_provider_error() {
        let registry = ProviderRegistry::new(); // No providers registered

        let spec = PackageSpec::new("owner/repo".parse().unwrap());

        let result = registry.resolve(&spec);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("No provider registered"));
    }

    #[test]
    fn test_resolve_from_meta_github() {
        let mut registry = ProviderRegistry::new();
        registry.register(make_mock_provider(ProviderKind::GitHub));

        let meta = Meta {
            name: "owner/repo".into(),
            api_url: "https://api.github.com".into(),
            ..Default::default()
        };

        let provider = registry.resolve_from_meta(&meta).unwrap();
        assert_eq!(provider.kind(), ProviderKind::GitHub);
    }

    #[test]
    fn test_resolve_from_meta_gitlab() {
        let mut registry = ProviderRegistry::new();
        registry.register(make_mock_provider(ProviderKind::GitLab));

        let meta = Meta {
            name: "owner/repo".into(),
            api_url: "https://gitlab.com/api/v4".into(),
            ..Default::default()
        };

        let provider = registry.resolve_from_meta(&meta).unwrap();
        assert_eq!(provider.kind(), ProviderKind::GitLab);
    }

    #[test]
    fn test_resolve_from_meta_gitee() {
        let mut registry = ProviderRegistry::new();
        registry.register(make_mock_provider(ProviderKind::Gitee));

        let meta = Meta {
            name: "owner/repo".into(),
            api_url: "https://gitee.com/api/v5".into(),
            ..Default::default()
        };

        let provider = registry.resolve_from_meta(&meta).unwrap();
        assert_eq!(provider.kind(), ProviderKind::Gitee);
    }

    #[test]
    fn test_resolve_from_meta_github_enterprise() {
        let mut registry = ProviderRegistry::new();
        registry.register(make_mock_provider(ProviderKind::GitHub));

        let meta = Meta {
            name: "owner/repo".into(),
            api_url: "https://github.mycompany.com/api/v3".into(),
            ..Default::default()
        };

        // GitHub Enterprise URLs don't contain "github" in subdomain pattern
        // but should still default to GitHub
        let provider = registry.resolve_from_meta(&meta).unwrap();
        assert_eq!(provider.kind(), ProviderKind::GitHub);
    }

    #[test]
    fn test_infer_provider_kind() {
        assert_eq!(
            ProviderRegistry::infer_provider_kind("https://api.github.com"),
            ProviderKind::GitHub
        );
        assert_eq!(
            ProviderRegistry::infer_provider_kind("https://gitlab.com/api/v4"),
            ProviderKind::GitLab
        );
        assert_eq!(
            ProviderRegistry::infer_provider_kind("https://gitee.com/api/v5"),
            ProviderKind::Gitee
        );
        // Unknown defaults to GitHub
        assert_eq!(
            ProviderRegistry::infer_provider_kind("https://unknown.com/api"),
            ProviderKind::GitHub
        );
    }

    #[test]
    fn test_package_spec_builder() {
        let spec = PackageSpec::new("owner/repo".parse().unwrap())
            .provider(ProviderKind::GitLab)
            .api_url("https://custom.gitlab.com/api/v4");

        assert_eq!(spec.repo.owner, "owner");
        assert_eq!(spec.repo.repo, "repo");
        assert_eq!(spec.provider_kind, Some(ProviderKind::GitLab));
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
        let mut registry = ProviderRegistry::new();
        registry.register(make_mock_provider(ProviderKind::GitHub));
        registry.register(make_mock_provider(ProviderKind::GitLab));

        let kinds = registry.registered_kinds();
        assert_eq!(kinds.len(), 2);
        assert!(kinds.contains(&ProviderKind::GitHub));
        assert!(kinds.contains(&ProviderKind::GitLab));
    }
}
