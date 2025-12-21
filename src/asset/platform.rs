/// Platform information for asset selection
#[derive(Debug, Clone, PartialEq)]
pub struct Platform {
    pub os: String,
    pub arch: String,
}

impl Platform {
    /// Detect the current platform
    pub fn detect() -> Self {
        Self {
            os: Self::detect_os(),
            arch: Self::detect_arch(),
        }
    }

    fn detect_os() -> String {
        #[cfg(target_os = "macos")]
        {
            "macos".to_string()
        }
        #[cfg(target_os = "linux")]
        {
            "linux".to_string()
        }
        #[cfg(target_os = "windows")]
        {
            "windows".to_string()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            std::env::consts::OS.to_string()
        }
    }

    fn detect_arch() -> String {
        #[cfg(target_arch = "x86_64")]
        {
            "x86_64".to_string()
        }
        #[cfg(target_arch = "aarch64")]
        {
            "aarch64".to_string()
        }
        #[cfg(target_arch = "x86")]
        {
            "i686".to_string()
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "x86")))]
        {
            std::env::consts::ARCH.to_string()
        }
    }
}

/// Trait for platform detection (useful for testing)
pub trait PlatformDetector: Send + Sync {
    fn detect(&self) -> Platform;
}

/// Default platform detector using compile-time detection
#[allow(dead_code)]
pub struct DefaultPlatformDetector;

impl PlatformDetector for DefaultPlatformDetector {
    fn detect(&self) -> Platform {
        Platform::detect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_detect() {
        let platform = Platform::detect();

        // Should return non-empty strings
        assert!(!platform.os.is_empty());
        assert!(!platform.arch.is_empty());

        // On known platforms, verify expected values
        #[cfg(target_os = "macos")]
        assert_eq!(platform.os, "macos");

        #[cfg(target_os = "linux")]
        assert_eq!(platform.os, "linux");

        #[cfg(target_os = "windows")]
        assert_eq!(platform.os, "windows");

        #[cfg(target_arch = "x86_64")]
        assert_eq!(platform.arch, "x86_64");

        #[cfg(target_arch = "aarch64")]
        assert_eq!(platform.arch, "aarch64");
    }

    #[test]
    fn test_default_platform_detector() {
        let detector = DefaultPlatformDetector;
        let platform = detector.detect();

        assert!(!platform.os.is_empty());
        assert!(!platform.arch.is_empty());
    }

    #[test]
    fn test_platform_clone_and_eq() {
        let p1 = Platform {
            os: "linux".into(),
            arch: "x86_64".into(),
        };
        let p2 = p1.clone();

        assert_eq!(p1, p2);
    }
}
