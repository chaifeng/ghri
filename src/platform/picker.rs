use crate::package::MetaAsset;

/// Trait for selecting an asset from a list of available assets
pub trait AssetPicker: Send + Sync {
    /// Pick the most appropriate asset from the given list
    ///
    /// Returns `None` if no suitable asset is found
    fn pick<'a>(&self, assets: &'a [MetaAsset]) -> Option<&'a MetaAsset>;
}

/// Default asset picker that uses platform detection to select assets
pub struct DefaultAssetPicker {
    platform: super::Platform,
}

impl DefaultAssetPicker {
    pub fn new() -> Self {
        Self {
            platform: super::Platform::detect(),
        }
    }

    pub fn with_platform(platform: super::Platform) -> Self {
        Self { platform }
    }

    /// Check if an asset name matches the current platform
    fn matches_platform(&self, name: &str) -> bool {
        let name_lower = name.to_lowercase();

        // Check OS match
        let os_match = match self.platform.os.as_str() {
            "macos" | "darwin" => {
                name_lower.contains("darwin")
                    || name_lower.contains("macos")
                    || name_lower.contains("apple")
            }
            "linux" => name_lower.contains("linux"),
            "windows" => name_lower.contains("windows") || name_lower.contains("win"),
            _ => false,
        };

        if !os_match {
            return false;
        }

        // Check architecture match
        match self.platform.arch.as_str() {
            "x86_64" | "amd64" => {
                name_lower.contains("x86_64")
                    || name_lower.contains("amd64")
                    || name_lower.contains("x64")
            }
            "aarch64" | "arm64" => name_lower.contains("aarch64") || name_lower.contains("arm64"),
            "i686" | "x86" => {
                name_lower.contains("i686")
                    || name_lower.contains("i386")
                    || name_lower.contains("x86") && !name_lower.contains("x86_64")
            }
            _ => true, // Allow if arch is unknown
        }
    }

    /// Score an asset for ranking (higher is better)
    fn score_asset(&self, name: &str) -> i32 {
        let name_lower = name.to_lowercase();
        let mut score = 0;

        // Prefer compressed archives
        if name_lower.ends_with(".tar.gz") || name_lower.ends_with(".tgz") {
            score += 10;
        } else if name_lower.ends_with(".zip") {
            score += 8;
        } else if name_lower.ends_with(".tar.xz") {
            score += 9;
        }

        // Penalize checksums and signatures
        if name_lower.contains("sha256")
            || name_lower.contains("sha512")
            || name_lower.contains("checksum")
        {
            score -= 100;
        }
        if name_lower.ends_with(".sig") || name_lower.ends_with(".asc") {
            score -= 100;
        }

        // Penalize source archives
        if name_lower.contains("source") || name_lower.contains("src") {
            score -= 50;
        }

        score
    }
}

impl Default for DefaultAssetPicker {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetPicker for DefaultAssetPicker {
    fn pick<'a>(&self, assets: &'a [MetaAsset]) -> Option<&'a MetaAsset> {
        let mut candidates: Vec<_> = assets
            .iter()
            .filter(|a| self.matches_platform(&a.name))
            .collect();

        if candidates.is_empty() {
            return None;
        }

        // Sort by score (descending)
        candidates.sort_by(|a, b| self.score_asset(&b.name).cmp(&self.score_asset(&a.name)));

        candidates.into_iter().next()
    }
}

/// A picker that always returns None (useful for testing or fallback to tarball)
#[allow(dead_code)]
pub struct NoOpAssetPicker;

impl AssetPicker for NoOpAssetPicker {
    fn pick<'a>(&self, _assets: &'a [MetaAsset]) -> Option<&'a MetaAsset> {
        None
    }
}

/// A picker that matches assets by a glob-like pattern
#[allow(dead_code)]
pub struct PatternAssetPicker {
    pattern: String,
}

#[allow(dead_code)]
impl PatternAssetPicker {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
        }
    }

    fn matches(&self, name: &str) -> bool {
        let pattern_lower = self.pattern.to_lowercase();
        let name_lower = name.to_lowercase();

        // Simple wildcard matching: * matches any characters
        if pattern_lower.contains('*') {
            let parts: Vec<&str> = pattern_lower.split('*').collect();
            let mut pos = 0;

            for (i, part) in parts.iter().enumerate() {
                if part.is_empty() {
                    continue;
                }

                if let Some(found) = name_lower[pos..].find(part) {
                    if i == 0 && found != 0 {
                        // First part must match at the beginning
                        return false;
                    }
                    pos += found + part.len();
                } else {
                    return false;
                }
            }

            // If pattern ends with *, allow any suffix
            // If not, the name must end where the pattern ends
            if !pattern_lower.ends_with('*') && pos != name_lower.len() {
                return false;
            }

            true
        } else {
            name_lower.contains(&pattern_lower)
        }
    }
}

impl AssetPicker for PatternAssetPicker {
    fn pick<'a>(&self, assets: &'a [MetaAsset]) -> Option<&'a MetaAsset> {
        assets.iter().find(|a| self.matches(&a.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper function to create test assets from names
    fn make_assets(names: &[&str]) -> Vec<MetaAsset> {
        names
            .iter()
            .map(|name| MetaAsset {
                name: name.to_string(),
                size: 1000,
                download_url: format!("https://example.com/{}", name),
            })
            .collect()
    }

    #[test]
    fn test_default_picker_linux_x86_64() {
        // Test that DefaultAssetPicker selects correct asset for Linux x86_64

        // --- Setup ---
        let picker = DefaultAssetPicker::with_platform(super::super::Platform {
            os: "linux".into(),
            arch: "x86_64".into(),
        });

        let assets = make_assets(&[
            "app-darwin-arm64.tar.gz",        // macOS ARM - not matching
            "app-linux-x86_64.tar.gz",        // Linux x86_64 - should match
            "app-windows-x64.zip",            // Windows - not matching
            "app-linux-x86_64.tar.gz.sha256", // Checksum file - should be penalized
        ]);

        // --- Execute & Verify ---
        let picked = picker.pick(&assets).unwrap();
        assert_eq!(picked.name, "app-linux-x86_64.tar.gz");
    }

    #[test]
    fn test_default_picker_macos_arm64() {
        // Test that DefaultAssetPicker selects correct asset for macOS ARM64

        // --- Setup ---
        let picker = DefaultAssetPicker::with_platform(super::super::Platform {
            os: "macos".into(),
            arch: "aarch64".into(),
        });

        let assets = make_assets(&[
            "app-darwin-arm64.tar.gz",  // macOS ARM - should match
            "app-darwin-x86_64.tar.gz", // macOS x86 - wrong arch
            "app-linux-x86_64.tar.gz",  // Linux - wrong OS
        ]);

        // --- Execute & Verify ---
        let picked = picker.pick(&assets).unwrap();
        assert_eq!(picked.name, "app-darwin-arm64.tar.gz");
    }

    #[test]
    fn test_default_picker_no_match() {
        // Test that DefaultAssetPicker returns None when no asset matches platform

        // --- Setup ---
        let picker = DefaultAssetPicker::with_platform(super::super::Platform {
            os: "freebsd".into(), // Unsupported OS
            arch: "x86_64".into(),
        });

        let assets = make_assets(&["app-darwin-arm64.tar.gz", "app-linux-x86_64.tar.gz"]);

        // --- Execute & Verify ---
        assert!(picker.pick(&assets).is_none());
    }

    #[test]
    fn test_default_picker_prefers_tar_gz() {
        // Test that DefaultAssetPicker prefers .tar.gz over .zip

        // --- Setup ---
        let picker = DefaultAssetPicker::with_platform(super::super::Platform {
            os: "linux".into(),
            arch: "x86_64".into(),
        });

        let assets = make_assets(&[
            "app-linux-x86_64.zip",    // Lower score
            "app-linux-x86_64.tar.gz", // Higher score (preferred)
        ]);

        // --- Execute & Verify ---
        let picked = picker.pick(&assets).unwrap();
        assert_eq!(picked.name, "app-linux-x86_64.tar.gz");
    }

    #[test]
    fn test_pattern_picker_simple() {
        // Test PatternAssetPicker with simple substring matching

        // --- Setup ---
        let picker = PatternAssetPicker::new("linux");

        let assets = make_assets(&[
            "app-darwin-arm64.tar.gz", // No "linux" in name
            "app-linux-x86_64.tar.gz", // Contains "linux"
        ]);

        // --- Execute & Verify ---
        let picked = picker.pick(&assets).unwrap();
        assert_eq!(picked.name, "app-linux-x86_64.tar.gz");
    }

    #[test]
    fn test_pattern_picker_wildcard() {
        // Test PatternAssetPicker with wildcard pattern matching

        // --- Setup ---
        let picker = PatternAssetPicker::new("app-*-x86_64.tar.gz");

        let assets = make_assets(&[
            "app-darwin-arm64.tar.gz", // Wrong arch
            "app-linux-x86_64.tar.gz", // Matches pattern
            "app-linux-arm64.tar.gz",  // Wrong arch
        ]);

        // --- Execute & Verify ---
        let picked = picker.pick(&assets).unwrap();
        assert_eq!(picked.name, "app-linux-x86_64.tar.gz");
    }

    #[test]
    fn test_noop_picker() {
        // Test that NoOpAssetPicker always returns None

        // --- Setup ---
        let picker = NoOpAssetPicker;
        let assets = make_assets(&["app.tar.gz"]);

        // --- Execute & Verify ---
        assert!(picker.pick(&assets).is_none());
    }

    #[test]
    fn test_score_prefers_archives_over_checksums() {
        // Test that archive files score higher than checksum/signature files

        // --- Setup ---
        let picker = DefaultAssetPicker::with_platform(super::super::Platform {
            os: "linux".into(),
            arch: "x86_64".into(),
        });

        // --- Verify Scoring ---

        // Archive files should score higher than checksums
        assert!(picker.score_asset("app.tar.gz") > picker.score_asset("app.tar.gz.sha256"));

        // Archive files should score higher than signatures
        assert!(picker.score_asset("app.zip") > picker.score_asset("app.zip.sig"));
    }
}
