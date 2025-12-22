#!/bin/sh
# ghri - GitHub Release Installer
# One-line installer: curl -fsSL https://raw.githubusercontent.com/chaifeng/ghri/main/install.sh | sh
# Or with custom bin path: curl -fsSL ... | sh -s -- /custom/bin/path
#
# POSIX shell compatible (dash, sh, bash, etc.)

set -e

# Colors (only if terminal supports them)
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    BLUE='\033[0;34m'
    NC='\033[0m' # No Color
else
    RED=''
    GREEN=''
    YELLOW=''
    BLUE=''
    NC=''
fi

info() {
    printf "${BLUE}[INFO]${NC} %s\n" "$1"
}

success() {
    printf "${GREEN}[OK]${NC} %s\n" "$1"
}

warn() {
    printf "${YELLOW}[WARN]${NC} %s\n" "$1"
}

error() {
    printf "${RED}[ERROR]${NC} %s\n" "$1" >&2
}

die() {
    error "$1"
    exit 1
}

# Check if command exists
has_cmd() {
    command -v "$1" >/dev/null 2>&1
}

# Detect download command (curl or wget)
detect_downloader() {
    if has_cmd curl; then
        DOWNLOADER="curl"
        DOWNLOAD_CMD="curl -fsSL"
        DOWNLOAD_OUTPUT="-o"
    elif has_cmd wget; then
        DOWNLOADER="wget"
        DOWNLOAD_CMD="wget -q"
        DOWNLOAD_OUTPUT="-O"
    else
        die "Neither curl nor wget found. Please install one of them."
    fi
}

# Download file
download() {
    url="$1"
    output="$2"
    $DOWNLOAD_CMD "$DOWNLOAD_OUTPUT" "$output" "$url"
}

# Download to stdout
download_stdout() {
    url="$1"
    if [ "$DOWNLOADER" = "curl" ]; then
        curl -fsSL "$url"
    else
        wget -qO- "$url"
    fi
}

# Detect OS and architecture
detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Darwin)
            OS_TYPE="apple-darwin"
            ARCHIVE_EXT="tar.gz"
            ;;
        Linux)
            OS_TYPE="unknown-linux-gnu"
            ARCHIVE_EXT="tar.gz"
            ;;
        MINGW*|MSYS*|CYGWIN*|Windows_NT)
            OS_TYPE="pc-windows-msvc"
            ARCHIVE_EXT="zip"
            ;;
        *)
            die "Unsupported operating system: $OS"
            ;;
    esac

    case "$ARCH" in
        x86_64|amd64)
            ARCH_TYPE="x86_64"
            ;;
        aarch64|arm64)
            ARCH_TYPE="aarch64"
            ;;
        *)
            die "Unsupported architecture: $ARCH"
            ;;
    esac

    PLATFORM="${ARCH_TYPE}-${OS_TYPE}"
    FILTER="*${PLATFORM}*"
}

# Find suitable bin directory
find_bin_dir() {
    # If user specified a path, use it
    if [ -n "$1" ]; then
        BIN_DIR="$1"
        return
    fi

    # Check common bin directories in order
    for dir in "/usr/local/bin" "$HOME/.local/bin" "$HOME/bin"; do
        # Check if directory exists
        if [ -d "$dir" ]; then
            # Check if writable
            if [ -w "$dir" ]; then
                # Check if in PATH
                case ":$PATH:" in
                    *":$dir:"*)
                        BIN_DIR="$dir"
                        return
                        ;;
                esac
            fi
        fi
    done

    # Default to ~/.local/bin
    BIN_DIR="$HOME/.local/bin"
    NEED_PATH_HINT=1
}

# Get latest version from GitHub
get_latest_version() {
    info "Fetching latest version..."
    RELEASE_JSON=$(download_stdout "https://api.github.com/repos/chaifeng/ghri/releases/latest")
    VERSION=$(printf '%s' "$RELEASE_JSON" | grep '"tag_name"' | cut -d'"' -f4)
    
    if [ -z "$VERSION" ]; then
        VERSION="v0.0.0"
        PUBLISHED_AT="2025-09-17T18:41:00Z"
        TARBALL_URL="https://github.com/chaifeng/ghri/tarball/${VERSION}"
        return
    fi

    # Get published_at
    PUBLISHED_AT=$(printf '%s' "$RELEASE_JSON" | grep '"published_at"' | head -1 | cut -d'"' -f4)
    
    # Get tarball_url (source code tarball)
    TARBALL_URL=$(printf '%s' "$RELEASE_JSON" | grep '"tarball_url"' | cut -d'"' -f4)

    success "Latest version: $VERSION"
}

# Get asset info for current platform
get_asset_info() {
    ASSET_NAME="ghri-${VERSION}-${PLATFORM}.${ARCHIVE_EXT}"
    DOWNLOAD_URL="https://github.com/chaifeng/ghri/releases/download/${VERSION}/${ASSET_NAME}"
    
    # Get asset size from release JSON
    # Find the asset matching our platform and extract its size
    ASSET_SIZE=$(printf '%s' "$RELEASE_JSON" | grep -A30 "\"name\": \"$ASSET_NAME\"" | grep '"size"' | head -1 | grep -o '[0-9]*')
    
    if [ -z "$ASSET_SIZE" ]; then
        ASSET_SIZE="unknown"
    fi
}

# Check if already installed
check_existing() {
    GHRI_ROOT="$HOME/.ghri"
    PACKAGE_DIR="$GHRI_ROOT/chaifeng/ghri"
    VERSION_DIR="$PACKAGE_DIR/$VERSION"
    META_FILE="$PACKAGE_DIR/meta.json"
    CURRENT_LINK="$PACKAGE_DIR/current"

    if [ -d "$VERSION_DIR" ] && [ -x "$VERSION_DIR/ghri" ]; then
        success "ghri $VERSION is already installed at $VERSION_DIR"
        exit 0
    fi
}

# Show installation plan and confirm
confirm_install() {
    printf "\n"
    # shellcheck disable=SC2059
    printf "${BLUE}=== ghri Installation Plan ===${NC}\n"
    printf "\n"
    printf "Version:      %s\n" "$VERSION"
    printf "Platform:     %s\n" "$PLATFORM"
    printf "Archive:      %s\n" "$ASSET_NAME"
    printf "Size:         %s bytes\n" "$ASSET_SIZE"
    printf "\n"
    # shellcheck disable=SC2059
    printf "${YELLOW}The following files/directories will be created or modified:${NC}\n"
    printf "\n"
    printf "  [DOWNLOAD] %s\n" "$DOWNLOAD_URL"
    printf "  [CREATE] %s/\n" "$VERSION_DIR"
    printf "  [CREATE] %s\n" "$META_FILE"
    printf "  [LINK]   %s -> %s\n" "$CURRENT_LINK" "$VERSION_DIR"
    printf "  [LINK]   %s/ghri -> %s/ghri\n" "$BIN_DIR" "$VERSION_DIR"
    printf "\n"

    if [ -n "$NEED_PATH_HINT" ]; then
        printf "${YELLOW}Note: %s is not in your PATH.${NC}\n" "$BIN_DIR"
        printf "After installation, add it to your PATH:\n"
        printf "\n"
        printf "  # For bash/zsh, add to ~/.bashrc or ~/.zshrc:\n"
        printf "  export PATH=\"%s:\$PATH\"\n" "$BIN_DIR"
        printf "\n"
    fi

    # Check if stdin is a terminal
    if [ -t 0 ]; then
        printf "Proceed with installation? [y/N] "
        read -r response
    else
        # When piped (e.g., curl | sh), try to read from /dev/tty
        if [ -e /dev/tty ]; then
            printf "Proceed with installation? [y/N] "
            read -r response < /dev/tty
        else
            # Fallback for non-interactive environments
            warn "Non-interactive mode detected, skipping confirmation"
            response="y"
        fi
    fi
    case "$response" in
        [yY][eE][sS]|[yY])
            return 0
            ;;
        *)
            info "Installation cancelled."
            exit 0
            ;;
    esac
}

# Create meta.json
create_meta_json() {
    # Get current timestamp
    NOW=$(date -u +"%Y-%m-%dT%H:%M:%SZ" 2>/dev/null || date +"%Y-%m-%dT%H:%M:%SZ")
    
    cat > "$META_FILE" << EOF
{
  "name": "chaifeng/ghri",
  "api_url": "https://api.github.com",
  "repo_info_url": "https://api.github.com/repos/chaifeng/ghri",
  "releases_url": "https://api.github.com/repos/chaifeng/ghri/releases",
  "description": null,
  "homepage": null,
  "license": "GNU General Public License v3.0",
  "updated_at": "${PUBLISHED_AT:-$NOW}",
  "current_version": "$VERSION",
  "releases": [
    {
      "version": "$VERSION",
      "title": "$VERSION",
      "published_at": "${PUBLISHED_AT:-$NOW}",
      "is_prerelease": false,
      "tarball_url": "$TARBALL_URL",
      "assets": [
        {
          "name": "$ASSET_NAME",
          "size": ${ASSET_SIZE:-0},
          "download_url": "$DOWNLOAD_URL"
        }
      ]
    }
  ],
  "links": [
    {
      "dest": "$BIN_DIR/ghri"
    }
  ],
  "filters": [
    "$FILTER"
  ]
}
EOF
}

# Perform installation
do_install() {
    # Create directories
    info "Creating directories..."
    mkdir -pv "$VERSION_DIR"
    mkdir -pv "$BIN_DIR"

    # Download archive
    info "Downloading $ASSET_NAME..."
    TEMP_FILE=$(mktemp)
    trap 'rm -f "$TEMP_FILE"' EXIT
    
    download "$DOWNLOAD_URL" "$TEMP_FILE"
    success "Download complete"

    # Extract archive
    info "Extracting archive..."
    case "$ARCHIVE_EXT" in
        tar.gz)
            tar -xzf "$TEMP_FILE" -C "$VERSION_DIR"
            ;;
        zip)
            if has_cmd unzip; then
                unzip -q "$TEMP_FILE" -d "$VERSION_DIR"
            else
                die "unzip command not found. Please install unzip."
            fi
            ;;
    esac
    success "Extraction complete"

    # Make binary executable
    chmod -v +x "$VERSION_DIR/ghri"

    # Create current symlink
    info "Creating symlinks..."
    rm -fv "$CURRENT_LINK"
    ln -sv "$VERSION" "$CURRENT_LINK"

    # Create bin symlink
    rm -fv "$BIN_DIR/ghri"
    ln -sv "$VERSION_DIR/ghri" "$BIN_DIR/ghri"

    # Create meta.json
    info "Creating metadata..."
    create_meta_json
    success "Metadata created"

    # Cleanup
    rm -f "$TEMP_FILE"
    trap - EXIT

    printf "\n"
    success "ghri $VERSION installed successfully!"
    printf "\n"

    if [ -n "$NEED_PATH_HINT" ]; then
        printf "${YELLOW}Remember to add %s to your PATH:${NC}\n" "$BIN_DIR"
        printf "  export PATH=\"\$HOME/.local/bin:\$PATH\"\n"
        printf "\n"
    fi

    printf "Run 'ghri --help' to get started.\n"
}

# Main
main() {
    # shellcheck disable=SC2059
    printf "${GREEN}ghri${NC} - GitHub Release Installer\n"
    printf "\n"

    # Parse arguments
    CUSTOM_BIN_DIR="$1"

    # Detect environment
    detect_downloader
    detect_platform
    
    info "Detected platform: $PLATFORM"
    info "Using downloader: $DOWNLOADER"

    # Get latest version
    get_latest_version
    get_asset_info

    # Find bin directory
    find_bin_dir "$CUSTOM_BIN_DIR"
    info "Binary will be linked to: $BIN_DIR"

    # Check if already installed
    check_existing

    # Confirm and install
    confirm_install
    do_install
}

main "$@"
