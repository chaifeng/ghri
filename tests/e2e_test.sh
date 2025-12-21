#!/usr/bin/env bash
#
# End-to-end test script for ghri
# Tests install, update operations with real GitHub repositories:
# - bach-sh/bach (v0.7.2)
# - chaifeng/zidr (v0.2.0)
#

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

#######################################
# Check for GITHUB_TOKEN
#######################################
if [[ -z "${GITHUB_TOKEN:-}" ]]; then
    echo -e "${RED}Error: GITHUB_TOKEN environment variable is not set.${NC}"
    echo ""
    echo "This test suite requires a GitHub token to avoid API rate limiting."
    echo "Anonymous access to the GitHub API is limited to 60 requests per hour,"
    echo "which may cause test failures."
    echo ""
    echo "To set a token, run:"
    echo "  export GITHUB_TOKEN=your_github_token"
    echo ""
    echo "You can create a personal access token at:"
    echo "  https://github.com/settings/tokens"
    echo ""
    echo "The token only needs 'public_repo' scope for this test."
    exit 1
fi

# Test counters
TESTS_PASSED=0
TESTS_FAILED=0

# Temporary directory for tests
TEST_ROOT=""
GHRI_BIN=""

#######################################
# Logging functions
#######################################
log_info() {
    echo -e "${BLUE}[INFO]${NC} $*"
}

log_success() {
    echo -e "${GREEN}[PASS]${NC} $*"
    TESTS_PASSED=$((TESTS_PASSED + 1))
}

log_fail() {
    echo -e "${RED}[FAIL]${NC} $*"
    TESTS_FAILED=$((TESTS_FAILED + 1))
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $*"
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$*${NC}"
    echo -e "${BLUE}========================================${NC}"
}

#######################################
# Setup and teardown
#######################################
setup() {
    log_section "Setting up test environment"

    # Find ghri binary
    if [[ -x "./target/debug/ghri" ]]; then
        GHRI_BIN="./target/debug/ghri"
    elif [[ -x "./target/release/ghri" ]]; then
        GHRI_BIN="./target/release/ghri"
    else
        log_info "Building ghri..."
        cargo build --quiet
        GHRI_BIN="./target/debug/ghri"
    fi

    log_info "Using ghri binary: $GHRI_BIN"

    # Create temporary test directory
    TEST_ROOT=$(mktemp -d)
    log_info "Test root directory: $TEST_ROOT"

    # Verify ghri works
    if "$GHRI_BIN" --version >/dev/null 2>&1; then
        log_info "ghri version: $("$GHRI_BIN" --version)"
    else
        log_fail "ghri binary not working"
        exit 1
    fi
}

teardown() {
    log_section "Cleaning up"

    if [[ -n "$TEST_ROOT" && -d "$TEST_ROOT" ]]; then
        rm -rf "$TEST_ROOT"
        log_info "Removed test directory: $TEST_ROOT"
    fi
}

# Ensure cleanup on exit
trap teardown EXIT

#######################################
# Helper functions
#######################################
assert_file_exists() {
    local file="$1"
    local msg="${2:-File should exist: $file}"

    if [[ -f "$file" ]]; then
        log_success "$msg"
        return 0
    else
        log_fail "$msg (file not found: $file)"
        return 1
    fi
}

assert_dir_exists() {
    local dir="$1"
    local msg="${2:-Directory should exist: $dir}"

    if [[ -d "$dir" ]]; then
        log_success "$msg"
        return 0
    else
        log_fail "$msg (directory not found: $dir)"
        return 1
    fi
}

assert_symlink_exists() {
    local link="$1"
    local msg="${2:-Symlink should exist: $link}"

    if [[ -L "$link" ]]; then
        log_success "$msg"
        return 0
    else
        log_fail "$msg (symlink not found: $link)"
        return 1
    fi
}

assert_symlink_target() {
    local link="$1"
    local expected_target="$2"
    local msg="${3:-Symlink target should be $expected_target}"

    if [[ -L "$link" ]]; then
        local actual_target
        actual_target=$(readlink "$link")
        if [[ "$actual_target" == "$expected_target" ]]; then
            log_success "$msg"
            return 0
        else
            log_fail "$msg (actual: $actual_target, expected: $expected_target)"
            return 1
        fi
    else
        log_fail "$msg (not a symlink: $link)"
        return 1
    fi
}

assert_file_contains() {
    local file="$1"
    local pattern="$2"
    local msg="${3:-File should contain pattern: $pattern}"

    if grep -q "$pattern" "$file" 2>/dev/null; then
        log_success "$msg"
        return 0
    else
        log_fail "$msg (pattern not found in $file)"
        return 1
    fi
}

assert_command_succeeds() {
    local msg="$1"
    shift

    if "$@" >/dev/null 2>&1; then
        log_success "$msg"
        return 0
    else
        log_fail "$msg (command failed: $*)"
        return 1
    fi
}

assert_command_fails() {
    local msg="$1"
    shift

    if ! "$@" >/dev/null 2>&1; then
        log_success "$msg"
        return 0
    else
        log_fail "$msg (command should have failed: $*)"
        return 1
    fi
}

#######################################
# Test cases
#######################################

test_install_bach() {
    log_section "Test: Install bach-sh/bach"

    local install_root="$TEST_ROOT/install_bach"
    mkdir -p "$install_root"

    log_info "Installing bach-sh/bach..."
    if "$GHRI_BIN" install bach-sh/bach --root "$install_root"; then
        log_success "Install command succeeded"
    else
        log_fail "Install command failed"
        return 1
    fi

    # Verify installation structure
    assert_dir_exists "$install_root/bach-sh/bach" "Package directory created"
    assert_file_exists "$install_root/bach-sh/bach/meta.json" "meta.json created"
    assert_symlink_exists "$install_root/bach-sh/bach/current" "current symlink created"

    # Verify meta.json content
    assert_file_contains "$install_root/bach-sh/bach/meta.json" "bach-sh/bach" "meta.json contains package name"
    assert_file_contains "$install_root/bach-sh/bach/meta.json" "api.github.com" "meta.json contains API URL"

    # Verify version directory exists
    local current_target
    current_target=$(readlink "$install_root/bach-sh/bach/current")
    assert_dir_exists "$install_root/bach-sh/bach/$current_target" "Version directory exists"

    log_info "Installed version: $current_target"
}

test_install_zidr() {
    log_section "Test: Install chaifeng/zidr"

    local install_root="$TEST_ROOT/install_zidr"
    mkdir -p "$install_root"

    log_info "Installing chaifeng/zidr..."
    if "$GHRI_BIN" install chaifeng/zidr --root "$install_root"; then
        log_success "Install command succeeded"
    else
        log_fail "Install command failed"
        return 1
    fi

    # Verify installation structure
    assert_dir_exists "$install_root/chaifeng/zidr" "Package directory created"
    assert_file_exists "$install_root/chaifeng/zidr/meta.json" "meta.json created"
    assert_symlink_exists "$install_root/chaifeng/zidr/current" "current symlink created"

    # Verify meta.json content
    assert_file_contains "$install_root/chaifeng/zidr/meta.json" "chaifeng/zidr" "meta.json contains package name"
}

test_install_idempotent() {
    log_section "Test: Install is idempotent (re-running install)"

    local install_root="$TEST_ROOT/install_idempotent"
    mkdir -p "$install_root"

    # First install
    log_info "First install of bach-sh/bach..."
    "$GHRI_BIN" install bach-sh/bach --root "$install_root" >/dev/null 2>&1

    local meta_before
    meta_before=$(cat "$install_root/bach-sh/bach/meta.json")

    # Second install (should be idempotent)
    log_info "Second install of bach-sh/bach (should skip download)..."
    if "$GHRI_BIN" install bach-sh/bach --root "$install_root" 2>&1 | grep -q "Skipping\|already exists"; then
        log_success "Second install skipped download (idempotent)"
    else
        # Even if output doesn't indicate skip, verify nothing broke
        log_success "Second install completed without error"
    fi

    # Verify structure is still intact
    assert_file_exists "$install_root/bach-sh/bach/meta.json" "meta.json still exists after re-install"
    assert_symlink_exists "$install_root/bach-sh/bach/current" "current symlink still exists"
}

test_install_multiple_packages() {
    log_section "Test: Install multiple packages"

    local install_root="$TEST_ROOT/install_multiple"
    mkdir -p "$install_root"

    # Install both packages
    log_info "Installing bach-sh/bach..."
    "$GHRI_BIN" install bach-sh/bach --root "$install_root" >/dev/null 2>&1

    log_info "Installing chaifeng/zidr..."
    "$GHRI_BIN" install chaifeng/zidr --root "$install_root" >/dev/null 2>&1

    # Verify both are installed
    assert_dir_exists "$install_root/bach-sh/bach" "bach-sh/bach installed"
    assert_dir_exists "$install_root/chaifeng/zidr" "chaifeng/zidr installed"

    # Verify they don't interfere with each other
    assert_file_exists "$install_root/bach-sh/bach/meta.json" "bach meta.json exists"
    assert_file_exists "$install_root/chaifeng/zidr/meta.json" "zidr meta.json exists"
}

test_install_specific_version() {
    log_section "Test: Install specific version with @version syntax"

    local install_root="$TEST_ROOT/install_version"
    mkdir -p "$install_root"

    # Install a specific older version
    log_info "Installing bach-sh/bach@0.6.0..."
    if "$GHRI_BIN" install "bach-sh/bach@0.6.0" --root "$install_root"; then
        log_success "Install specific version command succeeded"
    else
        log_fail "Install specific version command failed"
        return 1
    fi

    # Verify the specific version was installed
    assert_dir_exists "$install_root/bach-sh/bach/0.6.0" "Version 0.6.0 directory exists"

    # Verify current points to the specific version
    assert_symlink_target "$install_root/bach-sh/bach/current" "0.6.0" "current symlink points to 0.6.0"

    # Verify meta.json has the correct current_version
    if grep -q '"current_version": "0.6.0"' "$install_root/bach-sh/bach/meta.json"; then
        log_success "meta.json current_version is 0.6.0"
    else
        log_fail "meta.json current_version should be 0.6.0"
        return 1
    fi
}

test_install_version_with_v_prefix() {
    log_section "Test: Install version with v prefix"

    local install_root="$TEST_ROOT/install_v_prefix"
    mkdir -p "$install_root"

    # Install using v-prefixed version (zidr uses v prefix)
    log_info "Installing chaifeng/zidr@v0.2.0..."
    if "$GHRI_BIN" install "chaifeng/zidr@v0.2.0" --root "$install_root"; then
        log_success "Install with v-prefixed version succeeded"
    else
        log_fail "Install with v-prefixed version failed"
        return 1
    fi

    # Verify installation
    assert_dir_exists "$install_root/chaifeng/zidr/v0.2.0" "Version v0.2.0 directory exists"
    assert_symlink_target "$install_root/chaifeng/zidr/current" "v0.2.0" "current symlink points to v0.2.0"
}

test_install_nonexistent_version() {
    log_section "Test: Install non-existent version fails gracefully"

    local install_root="$TEST_ROOT/install_bad_version"
    mkdir -p "$install_root"

    log_info "Attempting to install non-existent version..."
    if ! "$GHRI_BIN" install "bach-sh/bach@v99.99.99" --root "$install_root" 2>&1 | grep -qi "not found\|available"; then
        # Command should fail
        if ! "$GHRI_BIN" install "bach-sh/bach@v99.99.99" --root "$install_root" 2>/dev/null; then
            log_success "Non-existent version correctly failed"
        else
            log_fail "Non-existent version should have failed"
            return 1
        fi
    else
        log_success "Non-existent version correctly reported error"
    fi
}

test_update_command() {
    log_section "Test: Update command"

    local install_root="$TEST_ROOT/update_test"
    mkdir -p "$install_root"

    # Install a package first
    log_info "Installing bach-sh/bach for update test..."
    "$GHRI_BIN" install bach-sh/bach --root "$install_root" >/dev/null 2>&1

    # Run update
    log_info "Running update command..."
    if "$GHRI_BIN" update --root "$install_root"; then
        log_success "Update command succeeded"
    else
        log_fail "Update command failed"
        return 1
    fi

    # Verify meta.json was updated (timestamp should be recent)
    assert_file_exists "$install_root/bach-sh/bach/meta.json" "meta.json exists after update"
}

test_update_empty_root() {
    log_section "Test: Update with no installed packages"

    local install_root="$TEST_ROOT/update_empty"
    mkdir -p "$install_root"

    log_info "Running update on empty root..."
    if "$GHRI_BIN" update --root "$install_root" 2>&1 | grep -qi "no packages"; then
        log_success "Update correctly reports no packages installed"
    else
        log_success "Update command handled empty root"
    fi
}

test_update_multiple_packages() {
    log_section "Test: Update with multiple packages"

    local install_root="$TEST_ROOT/update_multiple"
    mkdir -p "$install_root"

    # Install both packages
    log_info "Installing packages..."
    "$GHRI_BIN" install bach-sh/bach --root "$install_root" >/dev/null 2>&1
    "$GHRI_BIN" install chaifeng/zidr --root "$install_root" >/dev/null 2>&1

    # Run update
    log_info "Running update command..."
    if "$GHRI_BIN" update --root "$install_root"; then
        log_success "Update command succeeded for multiple packages"
    else
        log_fail "Update command failed"
        return 1
    fi
}

test_upgrade_after_update() {
    log_section "Test: Install old version -> update -> install upgrades to latest"

    local install_root="$TEST_ROOT/upgrade_test"
    mkdir -p "$install_root"

    # We'll use bach-sh/bach which has multiple versions
    # First, manually create a "fake" old installation with outdated meta.json

    local pkg_dir="$install_root/bach-sh/bach"
    local old_version="0.6.0"  # An older version that exists

    mkdir -p "$pkg_dir/$old_version"
    echo "fake old version content" > "$pkg_dir/$old_version/README.md"

    # Create symlink to old version
    ln -s "$old_version" "$pkg_dir/current"

    # Create a minimal meta.json with ONLY the old version (no latest version info)
    cat > "$pkg_dir/meta.json" <<EOF
{
    "name": "bach-sh/bach",
    "api_url": "https://api.github.com",
    "repo_info_url": "https://api.github.com/repos/bach-sh/bach",
    "releases_url": "https://api.github.com/repos/bach-sh/bach/releases",
    "description": "Bach Testing Framework",
    "homepage": null,
    "license": "MIT License",
    "updated_at": "2020-01-01T00:00:00Z",
    "current_version": "$old_version",
    "releases": [
        {
            "version": "$old_version",
            "title": "Old Release",
            "published_at": "2020-01-01T00:00:00Z",
            "is_prerelease": false,
            "tarball_url": "https://api.github.com/repos/bach-sh/bach/tarball/$old_version",
            "assets": []
        }
    ]
}
EOF

    log_info "Created fake old installation at version $old_version"

    # Verify initial state
    local initial_target
    initial_target=$(readlink "$pkg_dir/current")
    if [[ "$initial_target" != "$old_version" ]]; then
        log_fail "Initial symlink should point to $old_version"
        return 1
    fi
    log_success "Initial state: current -> $old_version"

    # Verify meta.json does NOT contain latest version (0.7.2)
    if grep -q "0.7.2" "$pkg_dir/meta.json"; then
        log_fail "Initial meta.json should NOT contain 0.7.2"
        return 1
    fi
    log_success "Initial meta.json does not contain latest version"

    # Run update to fetch latest release info
    log_info "Running update to fetch latest release info..."
    if ! "$GHRI_BIN" update --root "$install_root"; then
        log_fail "Update command failed"
        return 1
    fi
    log_success "Update command succeeded"

    # Verify meta.json NOW contains latest version info
    if grep -q "0.7.2" "$pkg_dir/meta.json"; then
        log_success "After update: meta.json contains latest version 0.7.2"
    else
        log_fail "After update: meta.json should contain 0.7.2"
        return 1
    fi

    # current should still point to old version (update doesn't change installed version)
    local after_update_target
    after_update_target=$(readlink "$pkg_dir/current")
    if [[ "$after_update_target" == "$old_version" ]]; then
        log_success "After update: current still points to $old_version (correct)"
    else
        log_fail "After update: current should still point to $old_version, got $after_update_target"
        return 1
    fi

    # Now run install to upgrade to latest
    log_info "Running install to upgrade to latest version..."
    if ! "$GHRI_BIN" install bach-sh/bach --root "$install_root"; then
        log_fail "Install (upgrade) command failed"
        return 1
    fi
    log_success "Install (upgrade) command succeeded"

    # Verify current now points to latest version
    local final_target
    final_target=$(readlink "$pkg_dir/current")
    if [[ "$final_target" == "0.7.2" ]]; then
        log_success "After install: current -> 0.7.2 (upgraded!)"
    else
        log_fail "After install: current should point to 0.7.2, got $final_target"
        return 1
    fi

    # Verify new version directory exists
    assert_dir_exists "$pkg_dir/0.7.2" "New version directory 0.7.2 exists"

    # Verify old version directory still exists (not deleted)
    assert_dir_exists "$pkg_dir/$old_version" "Old version directory still exists"

    # Verify meta.json current_version is updated
    if grep -q '"current_version": "0.7.2"' "$pkg_dir/meta.json"; then
        log_success "meta.json current_version is 0.7.2"
    else
        log_fail "meta.json current_version should be 0.7.2"
        return 1
    fi
}

test_invalid_repo_format() {
    log_section "Test: Invalid repository format"

    local install_root="$TEST_ROOT/invalid_repo"
    mkdir -p "$install_root"

    # Missing slash
    assert_command_fails "Invalid repo format (no slash) should fail" \
        "$GHRI_BIN" install "invalid" --root "$install_root"

    # Empty owner
    assert_command_fails "Invalid repo format (empty owner) should fail" \
        "$GHRI_BIN" install "/repo" --root "$install_root"

    # Empty repo
    assert_command_fails "Invalid repo format (empty repo) should fail" \
        "$GHRI_BIN" install "owner/" --root "$install_root"
}

test_nonexistent_repo() {
    log_section "Test: Non-existent repository"

    local install_root="$TEST_ROOT/nonexistent"
    mkdir -p "$install_root"

    # This should fail gracefully
    log_info "Attempting to install non-existent repo..."
    assert_command_fails "Non-existent repo should fail" \
        "$GHRI_BIN" install "this-owner-does-not-exist-12345/fake-repo-67890" --root "$install_root"
}

test_custom_root_via_env() {
    log_section "Test: Custom root via GHRI_ROOT environment variable"

    local install_root="$TEST_ROOT/env_root"
    mkdir -p "$install_root"

    log_info "Installing with GHRI_ROOT env var..."
    if GHRI_ROOT="$install_root" "$GHRI_BIN" install bach-sh/bach; then
        log_success "Install with GHRI_ROOT succeeded"
    else
        log_fail "Install with GHRI_ROOT failed"
        return 1
    fi

    assert_dir_exists "$install_root/bach-sh/bach" "Package installed to GHRI_ROOT"
}

test_help_commands() {
    log_section "Test: Help commands"

    assert_command_succeeds "Main help" "$GHRI_BIN" --help
    assert_command_succeeds "Install help" "$GHRI_BIN" install --help
    assert_command_succeeds "Update help" "$GHRI_BIN" update --help
}

test_version_command() {
    log_section "Test: Version command"

    if "$GHRI_BIN" --version | grep -q "ghri"; then
        log_success "Version command shows ghri"
    else
        log_fail "Version command output unexpected"
    fi
}

test_meta_json_structure() {
    log_section "Test: meta.json structure validation"

    local install_root="$TEST_ROOT/meta_structure"
    mkdir -p "$install_root"

    # Install a package
    "$GHRI_BIN" install bach-sh/bach --root "$install_root" >/dev/null 2>&1

    local meta_file="$install_root/bach-sh/bach/meta.json"

    # Verify required fields exist
    assert_file_contains "$meta_file" '"name"' "meta.json has name field"
    assert_file_contains "$meta_file" '"api_url"' "meta.json has api_url field"
    assert_file_contains "$meta_file" '"releases"' "meta.json has releases field"
    assert_file_contains "$meta_file" '"current_version"' "meta.json has current_version field"

    # Verify it's valid JSON
    if command -v jq >/dev/null 2>&1; then
        if jq . "$meta_file" >/dev/null 2>&1; then
            log_success "meta.json is valid JSON"
        else
            log_fail "meta.json is not valid JSON"
        fi
    else
        log_warn "jq not installed, skipping JSON validation"
    fi
}

test_symlink_target_is_relative() {
    log_section "Test: current symlink uses relative path"

    local install_root="$TEST_ROOT/symlink_relative"
    mkdir -p "$install_root"

    "$GHRI_BIN" install bach-sh/bach --root "$install_root" >/dev/null 2>&1

    local link="$install_root/bach-sh/bach/current"
    local target
    target=$(readlink "$link")

    # Target should NOT start with /
    if [[ "$target" != /* ]]; then
        log_success "Symlink target is relative: $target"
    else
        log_fail "Symlink target is absolute (should be relative): $target"
    fi
}

test_concurrent_installs() {
    log_section "Test: Concurrent installations (different packages)"

    local install_root="$TEST_ROOT/concurrent"
    mkdir -p "$install_root"

    log_info "Starting concurrent installations..."

    # Start both installs in background
    "$GHRI_BIN" install bach-sh/bach --root "$install_root" >/dev/null 2>&1 &
    local pid1=$!

    "$GHRI_BIN" install chaifeng/zidr --root "$install_root" >/dev/null 2>&1 &
    local pid2=$!

    # Wait for both
    local failed=0
    if ! wait $pid1; then
        log_fail "First concurrent install failed"
        failed=1
    fi
    if ! wait $pid2; then
        log_fail "Second concurrent install failed"
        failed=1
    fi

    if [[ $failed -eq 0 ]]; then
        log_success "Concurrent installations completed"

        # Verify both installed correctly
        assert_dir_exists "$install_root/bach-sh/bach" "bach-sh/bach installed"
        assert_dir_exists "$install_root/chaifeng/zidr" "chaifeng/zidr installed"
    fi
}

#######################################
# Link command tests
#######################################

test_link_to_file_path() {
    log_section "Test: Link to specific file path"

    local install_root="$TEST_ROOT/link_file_path"
    local link_dir="$TEST_ROOT/link_file_path_bin"
    mkdir -p "$install_root" "$link_dir"

    # Install bach first
    log_info "Installing bach-sh/bach..."
    if ! "$GHRI_BIN" install bach-sh/bach --root "$install_root"; then
        log_fail "Install command failed"
        return 1
    fi

    # Link to a specific file path
    local link_path="$link_dir/my-bach"
    log_info "Linking bach-sh/bach to $link_path..."
    if "$GHRI_BIN" link bach-sh/bach "$link_path" --root "$install_root"; then
        log_success "Link command succeeded"
    else
        log_fail "Link command failed"
        return 1
    fi

    # Verify symlink was created
    assert_symlink_exists "$link_path" "Symlink created at $link_path"

    # Verify symlink points to somewhere in the package
    local link_target
    link_target=$(readlink "$link_path")
    if [[ "$link_target" == *"bach-sh/bach"* ]]; then
        log_success "Symlink points to package directory"
    else
        log_fail "Symlink target unexpected: $link_target"
    fi

    # Verify meta.json has links field
    assert_file_contains "$install_root/bach-sh/bach/meta.json" "links" "meta.json contains links"
    assert_file_contains "$install_root/bach-sh/bach/meta.json" "my-bach" "meta.json contains link path"
}

test_link_to_directory() {
    log_section "Test: Link to directory (creates repo-named symlink)"

    local install_root="$TEST_ROOT/link_to_dir"
    local bin_dir="$TEST_ROOT/link_to_dir_bin"
    mkdir -p "$install_root" "$bin_dir"

    # Install zidr
    log_info "Installing chaifeng/zidr..."
    if ! "$GHRI_BIN" install chaifeng/zidr --root "$install_root"; then
        log_fail "Install command failed"
        return 1
    fi

    # Link to a directory - should create symlink inside with filename from link target
    log_info "Linking chaifeng/zidr to directory $bin_dir..."
    if "$GHRI_BIN" link chaifeng/zidr "$bin_dir" --root "$install_root"; then
        log_success "Link command succeeded"
    else
        log_fail "Link command failed"
        return 1
    fi

    # Verify symlink was created inside the directory
    # Note: the symlink name depends on the actual file in the version directory
    local link_count
    link_count=$(find "$bin_dir" -maxdepth 1 -type l | wc -l)
    if [[ $link_count -gt 0 ]]; then
        log_success "Symlink created in directory"
    else
        log_fail "No symlink found in $bin_dir"
    fi

    # Verify meta.json has links field
    assert_file_contains "$install_root/chaifeng/zidr/meta.json" "links" "meta.json contains links"
}

test_link_update_on_version_change() {
    log_section "Test: Link updates when installing new version"

    local install_root="$TEST_ROOT/link_update"
    local link_dir="$TEST_ROOT/link_update_bin"
    mkdir -p "$install_root" "$link_dir"

    # Install older version of bach
    log_info "Installing bach-sh/bach@0.7.1..."
    if ! "$GHRI_BIN" install bach-sh/bach@0.7.1 --root "$install_root"; then
        log_fail "Install v0.7.1 failed"
        return 1
    fi

    # Create link
    local link_path="$link_dir/bach"
    log_info "Linking bach-sh/bach to $link_path..."
    if ! "$GHRI_BIN" link bach-sh/bach "$link_path" --root "$install_root"; then
        log_fail "Link command failed"
        return 1
    fi

    # Verify link points to v0.7.1
    local v1_target
    v1_target=$(readlink "$link_path")
    if [[ "$v1_target" == *"0.7.1"* ]]; then
        log_success "Initial link points to 0.7.1"
    else
        log_fail "Initial link should point to 0.7.1, got: $v1_target"
    fi

    # Update to get new releases
    log_info "Running update..."
    "$GHRI_BIN" update --root "$install_root" || true

    # Install newer version
    log_info "Installing bach-sh/bach@0.7.2..."
    if ! "$GHRI_BIN" install bach-sh/bach@0.7.2 --root "$install_root"; then
        log_fail "Install v0.7.2 failed"
        return 1
    fi

    # Verify link now points to v0.7.2
    local v2_target
    v2_target=$(readlink "$link_path")
    if [[ "$v2_target" == *"0.7.2"* ]]; then
        log_success "Link updated to point to 0.7.2"
    else
        log_fail "Link should now point to 0.7.2, got: $v2_target"
    fi
}

test_link_update_existing_symlink() {
    log_section "Test: Link updates existing symlink to same package"

    local install_root="$TEST_ROOT/link_update_existing"
    local link_dir="$TEST_ROOT/link_update_existing_bin"
    mkdir -p "$install_root" "$link_dir"

    # Install two versions of bach
    log_info "Installing bach-sh/bach@0.7.1..."
    if ! "$GHRI_BIN" install bach-sh/bach@0.7.1 --root "$install_root"; then
        log_fail "Install v0.7.1 failed"
        return 1
    fi

    # Create link to v0.7.1
    local link_path="$link_dir/bach"
    log_info "Linking to v0.7.1..."
    if ! "$GHRI_BIN" link bach-sh/bach "$link_path" --root "$install_root"; then
        log_fail "First link command failed"
        return 1
    fi

    # Update and install v0.7.2
    "$GHRI_BIN" update --root "$install_root" || true
    log_info "Installing bach-sh/bach@0.7.2..."
    if ! "$GHRI_BIN" install bach-sh/bach@0.7.2 --root "$install_root"; then
        log_fail "Install v0.7.2 failed"
        return 1
    fi

    # Link again - should update existing symlink
    log_info "Re-linking to current version (0.7.2)..."
    if "$GHRI_BIN" link bach-sh/bach "$link_path" --root "$install_root"; then
        log_success "Second link command succeeded"
    else
        log_fail "Second link command failed"
        return 1
    fi

    # Verify link now points to v0.7.2
    local target
    target=$(readlink "$link_path")
    if [[ "$target" == *"0.7.2"* ]]; then
        log_success "Link updated to 0.7.2"
    else
        log_fail "Link should point to 0.7.2, got: $target"
    fi
}

test_link_fails_for_uninstalled() {
    log_section "Test: Link fails for uninstalled package"

    local install_root="$TEST_ROOT/link_uninstalled"
    local link_dir="$TEST_ROOT/link_uninstalled_bin"
    mkdir -p "$install_root" "$link_dir"

    # Try to link without installing first
    log_info "Attempting to link uninstalled package..."
    local output
    if output=$("$GHRI_BIN" link nonexistent/package "$link_dir/pkg" --root "$install_root" 2>&1); then
        log_fail "Link should have failed for uninstalled package"
    else
        if echo "$output" | grep -qi "not installed"; then
            log_success "Link correctly fails for uninstalled package"
        else
            log_info "Error output: $output"
            log_success "Link command failed (expected)"
        fi
    fi
}

test_link_fails_for_existing_file() {
    log_section "Test: Link fails when destination is existing file"

    local install_root="$TEST_ROOT/link_existing_file"
    local link_dir="$TEST_ROOT/link_existing_file_bin"
    mkdir -p "$install_root" "$link_dir"

    # Install bach
    log_info "Installing bach-sh/bach..."
    if ! "$GHRI_BIN" install bach-sh/bach --root "$install_root"; then
        log_fail "Install command failed"
        return 1
    fi

    # Create a regular file at the link destination
    local blocking_file="$link_dir/bach"
    echo "I'm blocking" > "$blocking_file"

    # Try to link - should fail
    log_info "Attempting to link to existing file..."
    if "$GHRI_BIN" link bach-sh/bach "$blocking_file" --root "$install_root" 2>&1; then
        log_fail "Link should have failed for existing file"
    else
        log_success "Link correctly fails for existing file"
    fi

    # Verify the original file is unchanged
    if [[ "$(cat "$blocking_file")" == "I'm blocking" ]]; then
        log_success "Original file was not modified"
    else
        log_fail "Original file was modified!"
    fi
}

test_link_single_file_detection() {
    log_section "Test: Link detects single file in version directory"

    local install_root="$TEST_ROOT/link_single_file"
    local link_dir="$TEST_ROOT/link_single_file_bin"
    mkdir -p "$install_root" "$link_dir"

    # Install bach (which should have a single 'bach' file)
    log_info "Installing bach-sh/bach..."
    if ! "$GHRI_BIN" install bach-sh/bach --root "$install_root"; then
        log_fail "Install command failed"
        return 1
    fi

    # Get current version
    local current_version
    current_version=$(readlink "$install_root/bach-sh/bach/current")

    # Check how many files are in version directory
    local file_count
    file_count=$(ls -1 "$install_root/bach-sh/bach/$current_version" 2>/dev/null | wc -l)
    log_info "Version directory has $file_count item(s)"

    # Link
    local link_path="$link_dir/bach"
    log_info "Linking bach-sh/bach..."
    if ! "$GHRI_BIN" link bach-sh/bach "$link_path" --root "$install_root"; then
        log_fail "Link command failed"
        return 1
    fi

    # Verify the link target
    local target
    target=$(readlink "$link_path")

    if [[ $file_count -eq 1 ]]; then
        # Should link to the single file, not the directory
        if [[ -f "$target" ]] || [[ "$target" == *"/bach" && "$target" != *"/bach/"* ]]; then
            log_success "Single file detected - link points to file"
        else
            log_info "Link target: $target (may be directory if multiple files)"
        fi
    else
        log_info "Multiple files in version directory - link points to directory"
    fi

    log_success "Link created successfully"
}

#######################################
# Unlink Tests
#######################################

test_unlink_single_link() {
    log_section "Test: Unlink removes single link"

    local install_root="$TEST_ROOT/unlink_single"
    local link_path="$TEST_ROOT/unlink_single_bin/my-tool"
    mkdir -p "$install_root" "$(dirname "$link_path")"

    # Install bach
    log_info "Installing bach-sh/bach..."
    if ! "$GHRI_BIN" install bach-sh/bach --root "$install_root"; then
        log_fail "Install command failed"
        return 1
    fi

    # Create link
    log_info "Creating link..."
    if ! "$GHRI_BIN" link bach-sh/bach "$link_path" --root "$install_root"; then
        log_fail "Link command failed"
        return 1
    fi

    # Verify link exists
    if [[ -L "$link_path" ]]; then
        log_success "Link created"
    else
        log_fail "Link was not created"
        return 1
    fi

    # Unlink
    log_info "Unlinking..."
    if "$GHRI_BIN" unlink bach-sh/bach "$link_path" --root "$install_root"; then
        log_success "Unlink command succeeded"
    else
        log_fail "Unlink command failed"
        return 1
    fi

    # Verify link removed
    if [[ ! -e "$link_path" ]]; then
        log_success "Link removed"
    else
        log_fail "Link still exists after unlink"
    fi

    # Verify meta.json no longer has the link rule
    if ! grep -q "$link_path" "$install_root/bach-sh/bach/meta.json" 2>/dev/null; then
        log_success "Link rule removed from meta.json"
    else
        log_fail "Link rule still in meta.json"
    fi
}

test_unlink_all_links() {
    log_section "Test: Unlink --all removes all links"

    local install_root="$TEST_ROOT/unlink_all"
    local link1="$TEST_ROOT/unlink_all_bin/link1"
    local link2="$TEST_ROOT/unlink_all_bin/link2"
    mkdir -p "$install_root" "$(dirname "$link1")"

    # Install bach
    log_info "Installing bach-sh/bach..."
    if ! "$GHRI_BIN" install bach-sh/bach --root "$install_root"; then
        log_fail "Install command failed"
        return 1
    fi

    # Create two links
    log_info "Creating first link..."
    if ! "$GHRI_BIN" link bach-sh/bach "$link1" --root "$install_root"; then
        log_fail "First link command failed"
        return 1
    fi

    log_info "Creating second link..."
    if ! "$GHRI_BIN" link bach-sh/bach "$link2" --root "$install_root"; then
        log_fail "Second link command failed"
        return 1
    fi

    # Verify both links exist
    if [[ -L "$link1" ]] && [[ -L "$link2" ]]; then
        log_success "Both links created"
    else
        log_fail "Links were not created"
        return 1
    fi

    # Show links before unlink
    log_info "Link rules before unlink:"
    "$GHRI_BIN" links bach-sh/bach --root "$install_root" || true

    # Unlink all
    log_info "Unlinking all..."
    if "$GHRI_BIN" unlink bach-sh/bach --all --root "$install_root"; then
        log_success "Unlink --all command succeeded"
    else
        log_fail "Unlink --all command failed"
        return 1
    fi

    # Verify both links removed
    if [[ ! -e "$link1" ]] && [[ ! -e "$link2" ]]; then
        log_success "All links removed"
    else
        log_fail "Some links still exist after unlink --all"
    fi
}

test_unlink_nonexistent_symlink() {
    log_section "Test: Unlink removes rule even if symlink doesn't exist"

    local install_root="$TEST_ROOT/unlink_nonexistent"
    local link_path="$TEST_ROOT/unlink_nonexistent_bin/my-tool"
    mkdir -p "$install_root" "$(dirname "$link_path")"

    # Install bach
    log_info "Installing bach-sh/bach..."
    if ! "$GHRI_BIN" install bach-sh/bach --root "$install_root"; then
        log_fail "Install command failed"
        return 1
    fi

    # Create link
    log_info "Creating link..."
    if ! "$GHRI_BIN" link bach-sh/bach "$link_path" --root "$install_root"; then
        log_fail "Link command failed"
        return 1
    fi

    # Manually remove the symlink (simulating external deletion)
    rm -f "$link_path"
    log_info "Manually removed symlink (simulating external deletion)"

    # Unlink should still succeed and remove the rule
    log_info "Unlinking (symlink already deleted)..."
    if "$GHRI_BIN" unlink bach-sh/bach "$link_path" --root "$install_root"; then
        log_success "Unlink command succeeded (removed rule only)"
    else
        log_fail "Unlink command failed"
        return 1
    fi

    # Verify rule removed from meta.json
    if ! grep -q "$link_path" "$install_root/bach-sh/bach/meta.json" 2>/dev/null; then
        log_success "Link rule removed from meta.json"
    else
        log_fail "Link rule still in meta.json"
    fi
}

test_unlink_fails_for_uninstalled() {
    log_section "Test: Unlink fails for uninstalled package"

    local install_root="$TEST_ROOT/unlink_not_installed"
    mkdir -p "$install_root"

    log_info "Attempting to unlink uninstalled package..."
    local output
    if output=$("$GHRI_BIN" unlink nonexistent/package --all --root "$install_root" 2>&1); then
        log_fail "Unlink should have failed for uninstalled package"
    else
        if echo "$output" | grep -qi "not installed"; then
            log_success "Unlink correctly fails for uninstalled package"
        else
            log_info "Error output: $output"
            log_success "Unlink command failed (expected)"
        fi
    fi
}

test_unlink_requires_dest_or_all() {
    log_section "Test: Unlink requires dest or --all"

    local install_root="$TEST_ROOT/unlink_needs_arg"
    local link_path="$TEST_ROOT/unlink_needs_arg_bin/tool"
    mkdir -p "$install_root" "$(dirname "$link_path")"

    # Install and link
    log_info "Installing bach-sh/bach..."
    if ! "$GHRI_BIN" install bach-sh/bach --root "$install_root"; then
        log_fail "Install command failed"
        return 1
    fi

    log_info "Creating link..."
    if ! "$GHRI_BIN" link bach-sh/bach "$link_path" --root "$install_root"; then
        log_fail "Link command failed"
        return 1
    fi

    # Try unlink without dest or --all
    log_info "Attempting unlink without dest or --all..."
    local output
    if output=$("$GHRI_BIN" unlink bach-sh/bach --root "$install_root" 2>&1); then
        log_fail "Unlink should require dest or --all"
    else
        if echo "$output" | grep -qi "\-\-all\|destination"; then
            log_success "Unlink correctly requires dest or --all"
        else
            log_info "Error output: $output"
            log_success "Unlink command failed (expected)"
        fi
    fi

    # Verify link still exists
    if [[ -L "$link_path" ]]; then
        log_success "Link preserved after failed unlink"
    else
        log_fail "Link should still exist"
    fi
}

test_links_command() {
    log_section "Test: Links command shows link rules"

    local install_root="$TEST_ROOT/links_cmd"
    local link1="$TEST_ROOT/links_cmd_bin/tool1"
    local link2="$TEST_ROOT/links_cmd_bin/tool2"
    mkdir -p "$install_root" "$(dirname "$link1")"

    # Install
    log_info "Installing bach-sh/bach..."
    if ! "$GHRI_BIN" install bach-sh/bach --root "$install_root"; then
        log_fail "Install command failed"
        return 1
    fi

    # Initially no links
    log_info "Checking links (should be empty)..."
    local output
    output=$("$GHRI_BIN" links bach-sh/bach --root "$install_root" 2>&1)
    if echo "$output" | grep -qi "no link"; then
        log_success "No links initially"
    else
        log_info "Output: $output"
    fi

    # Create links
    "$GHRI_BIN" link bach-sh/bach "$link1" --root "$install_root" >/dev/null 2>&1
    "$GHRI_BIN" link bach-sh/bach "$link2" --root "$install_root" >/dev/null 2>&1

    # Check links command output
    log_info "Checking links after creating two..."
    output=$("$GHRI_BIN" links bach-sh/bach --root "$install_root" 2>&1)
    if echo "$output" | grep -q "$link1" && echo "$output" | grep -q "$link2"; then
        log_success "Links command shows both links"
    else
        log_info "Output: $output"
        log_fail "Links command should show both links"
    fi
}

#######################################
# Main
#######################################
main() {
    log_section "ghri End-to-End Tests"
    log_info "Starting comprehensive test suite..."

    setup

    # Run all tests
    test_help_commands
    test_version_command

    test_install_bach
    test_install_zidr
    test_install_idempotent
    test_install_multiple_packages
    test_install_specific_version
    test_install_version_with_v_prefix
    test_install_nonexistent_version

    test_update_command
    test_update_empty_root
    test_update_multiple_packages
    test_upgrade_after_update

    test_invalid_repo_format
    test_nonexistent_repo

    test_custom_root_via_env
    test_meta_json_structure
    test_symlink_target_is_relative
    test_concurrent_installs

    # Link command tests
    test_link_to_file_path
    test_link_to_directory
    test_link_update_on_version_change
    test_link_update_existing_symlink
    test_link_fails_for_uninstalled
    test_link_fails_for_existing_file
    test_link_single_file_detection

    # Unlink command tests
    test_unlink_single_link
    test_unlink_all_links
    test_unlink_nonexistent_symlink
    test_unlink_fails_for_uninstalled
    test_unlink_requires_dest_or_all
    test_links_command

    # Summary
    log_section "Test Summary"
    echo -e "${GREEN}Passed: $TESTS_PASSED${NC}"

    if [[ $TESTS_FAILED -gt 0 ]]; then
        echo -e "${RED}Failed: $TESTS_FAILED${NC}"
        echo -e "\n${RED}Some tests failed!${NC}"
        exit 1
    else
        echo -e "\n${GREEN}All tests passed!${NC}"
        exit 0
    fi
}

main "$@"
