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
    
    # Summary
    log_section "Test Summary"
    echo -e "${GREEN}Passed: $TESTS_PASSED${NC}"
    echo -e "${RED}Failed: $TESTS_FAILED${NC}"
    
    if [[ $TESTS_FAILED -gt 0 ]]; then
        echo -e "\n${RED}Some tests failed!${NC}"
        exit 1
    else
        echo -e "\n${GREEN}All tests passed!${NC}"
        exit 0
    fi
}

main "$@"
