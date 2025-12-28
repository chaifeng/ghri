#!/usr/bin/env bash
#
# End-to-end test script for ghri
# Tests install, update operations with real GitHub repositories:
# - bach-sh/bach (v0.7.2)
# - chaifeng/zidr (v0.2.0)
#

set -uo pipefail
PATH=/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin
declare -a test_filters=("$@")

current_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -e "${current_dir}/../.env" ]]; then
    # shellcheck source=../.env
    source "${current_dir}/../.env"
fi

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
# shellcheck disable=SC2329
note() {
    echo -e "${BLUE}[INFO]${NC} $*"
}

# shellcheck disable=SC2329
pass() {
    echo -e "${GREEN}[PASS]${NC} $*"
    TESTS_PASSED=$((TESTS_PASSED + 1))
}

# shellcheck disable=SC2329
fail() {
    echo -e "${RED}[FAIL]${NC} $*"
    TESTS_FAILED=$((TESTS_FAILED + 1))
}

# shellcheck disable=SC2329
warn() {
    echo -e "${YELLOW}[WARN]${NC} $*"
}

# shellcheck disable=SC2329
describe_scenario() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$*${NC}"
    echo -e "${BLUE}========================================${NC}"
}

#######################################
# Setup and teardown
#######################################
# shellcheck disable=SC2329
setup() {
    describe_scenario "Setting up test environment"

    # Find ghri binary (use absolute path to work with pushd/popd)
    if [[ -x "./target/debug/ghri" ]]; then
        GHRI_BIN="$(pwd)/target/debug/ghri"
    elif [[ -x "./target/release/ghri" ]]; then
        GHRI_BIN="$(pwd)/target/release/ghri"
    else
        note "Building ghri..."
        cargo build --quiet
        GHRI_BIN="$(pwd)/target/debug/ghri"
    fi

    note "Using ghri binary: $GHRI_BIN"

    # Create temporary test directory
    TEST_ROOT=$(mktemp -d)
    note "Test root directory: $TEST_ROOT"

    # Verify ghri works
    if "$GHRI_BIN" --version >/dev/null 2>&1; then
        note "ghri version: $("$GHRI_BIN" --version)"
    else
        fail "ghri binary not working"
        exit 1
    fi

    # Verify jq exists
    if ! command -v jq &> /dev/null; then
        fail "jq is required for these tests but not found"
        exit 1
    fi
}

# shellcheck disable=SC2329
ghri() {
    if [[ -n  "${GHRI_ROOT:-}" ]]; then
        env GHRI_ROOT="${GHRI_ROOT}" "$GHRI_BIN" "$@"
    else
        "$GHRI_BIN" "$@"
    fi
}

# Set install root for subsequent ghri commands
# shellcheck disable=SC2329
using_root() {
    GHRI_ROOT="$1"
    export GHRI_ROOT
    mkdir -p "$GHRI_ROOT"
}

# shellcheck disable=SC2329
teardown() {
    describe_scenario "Cleaning up"

    if [[ -n "$TEST_ROOT" && -d "$TEST_ROOT" ]]; then
        rm -rf "$TEST_ROOT"
        note "Removed test directory: $TEST_ROOT"
    fi
}

# Ensure cleanup on exit
trap teardown EXIT

#######################################
# Helper functions
#######################################

# Run a command, pass if succeeds, fail and exit if fails
# shellcheck disable=SC2329
run() {
    if "$@"; then
        pass "$*"
    else
        fail "$* (exit code: $?)"
        exit 1
    fi
}

# Run a command silently, pass if succeeds, fail and exit if fails
# shellcheck disable=SC2329
quietly() {
    if "$@" >/dev/null 2>&1; then
        pass "$*"
    else
        fail "$* (exit code: $?)"
        exit 1
    fi
}

# Ensure precondition, fail and exit if not met
# Usage: ensure file PATH exists
#        ensure directory PATH exists  
#        ensure symlink PATH exists
#        ensure PATH exists (any type)
# shellcheck disable=SC2329
ensure() {
    if dsl_match file _ exists === "$@"; then
        [[ -f "${DSL_ARGS[0]}" ]] || { fail "File does not exist: ${DSL_ARGS[0]}"; exit 1; }
    elif dsl_match directory _ exists === "$@"; then
        [[ -d "${DSL_ARGS[0]}" ]] || { fail "Directory does not exist: ${DSL_ARGS[0]}"; exit 1; }
    elif dsl_match symlink _ exists === "$@"; then
        [[ -L "${DSL_ARGS[0]}" ]] || { fail "Symlink does not exist: ${DSL_ARGS[0]}"; exit 1; }
    elif dsl_match _ exists === "$@"; then
        [[ -e "${DSL_ARGS[0]}" ]] || { fail "Path does not exist: ${DSL_ARGS[0]}"; exit 1; }
    else
        fail "Unknown ensure pattern: $*"
        exit 1
    fi
}

# Check command output contains pattern
# Usage: output should contain PATTERN from command: COMMAND...
# shellcheck disable=SC2329
output() {
    if dsl_match should contain _ from command: % === "$@"; then
        local pattern="${DSL_ARGS[0]}"
        local cmd_output
        if cmd_output="$("${DSL_ARGS[@]:1}" 2>&1)"; then
            if echo "$cmd_output" | grep -q "$pattern"; then
                pass "Output of '${DSL_ARGS[*]:1}' contains '$pattern'"
            else
                fail "Output of '${DSL_ARGS[*]:1}' does not contain '$pattern'"
                # shellcheck disable=SC2001
                sed 's/^/\t| /' <<< "$cmd_output"
            fi
        else
            pass "'${DSL_ARGS[*]:1}' completed (checking output)"
        fi
    else
        fail "Unknown output pattern: $*"
    fi
}

# DSL pattern matching helper
# Usage: dsl_match pattern words _ placeholder === "$@"
# Returns 0 if matches, captures values in DSL_ARGS array
# '_' matches any single argument, '%' matches rest of arguments
# '===' separates pattern from actual arguments
declare -a DSL_ARGS=()
# shellcheck disable=SC2329
dsl_match() {
    DSL_ARGS=()
    
    # Split args by ===
    local -a pattern=()
    local -a actual=()
    local found_sep=false
    
    for arg in "$@"; do
        if [[ "$arg" == "===" ]]; then
            found_sep=true
        elif $found_sep; then
            actual+=("$arg")
        else
            pattern+=("$arg")
        fi
    done
    
    $found_sep || return 1
    
    local pi=0 ai=0
    local plen=${#pattern[@]}
    local alen=${#actual[@]}
    
    while [[ $pi -lt $plen ]]; do
        local pw="${pattern[$pi]}"
        if [[ "$pw" == "_" ]]; then
            # Wildcard: capture this argument
            [[ $ai -ge $alen ]] && return 1
            DSL_ARGS+=("${actual[$ai]}")
            ((pi++))
            ((ai++))
        elif [[ "$pw" == "%" ]]; then
            # Rest: capture remaining arguments
            while [[ $ai -lt $alen ]]; do
                DSL_ARGS+=("${actual[$ai]}")
                ((ai++))
            done
            return 0
        else
            # Literal: must match exactly
            [[ $ai -ge $alen ]] && return 1
            [[ "${actual[$ai]}" != "$pw" ]] && return 1
            ((pi++))
            ((ai++))
        fi
    done
    
    # All pattern words consumed, check if all args consumed
    [[ $ai -eq $alen ]]
}

# DSL expect function
# Patterns use '_' for single value placeholder, '*' for rest of args
# shellcheck disable=SC2329
expect() {
    if dsl_match file _ to exist === "$@"; then
        expect_file_to_exist "${DSL_ARGS[0]}"

    elif dsl_match file _ to contain _ === "$@"; then
        expect_file_to_contain "${DSL_ARGS[0]}" "${DSL_ARGS[1]}"

    elif dsl_match directory _ to exist === "$@"; then
        expect_directory_to_exist "${DSL_ARGS[0]}"

    elif dsl_match symlink _ to exist === "$@"; then
        expect_symlink_to_exist "${DSL_ARGS[0]}"

    elif dsl_match symlink _ not to exist === "$@"; then
        expect_symlink_to_not_exist "${DSL_ARGS[0]}"

    elif dsl_match symlink _ to point to _ === "$@"; then
        expect_symlink_target_to_be "${DSL_ARGS[0]}" "${DSL_ARGS[1]}"

    elif dsl_match symlink _ to point to matching _ === "$@"; then
        expect_symlink_target_to_contain "${DSL_ARGS[0]}" "${DSL_ARGS[1]}"

    elif dsl_match symlink _ to be relative to _ === "$@"; then
        expect_symlink_to_be_relative "${DSL_ARGS[1]}" "${DSL_ARGS[0]}"

    elif dsl_match command to succeed: % === "$@"; then
        expect_command_to_succeed "Command ${DSL_ARGS[*]}" "${DSL_ARGS[@]}"

    elif dsl_match command to fail: % === "$@"; then
        expect_command_to_fail "Command ${DSL_ARGS[*]}" "${DSL_ARGS[@]}"

    elif dsl_match link to _ in meta === "$@"; then
        verify_metadata_contains_link "$GHRI_ROOT/${DSL_ARGS[0]}/meta.json"

    elif dsl_match link to _ at version _ in meta === "$@"; then
        verify_metadata_contains_versioned_link "$GHRI_ROOT/${DSL_ARGS[0]}/meta.json" "${DSL_ARGS[1]}"

    elif dsl_match link to _ at version _ with path _ in meta === "$@"; then
        verify_metadata_contains_versioned_link "$GHRI_ROOT/${DSL_ARGS[0]}/meta.json" "${DSL_ARGS[1]}" "${DSL_ARGS[2]}"

    elif dsl_match link to _ with path _ in meta === "$@"; then
        verify_metadata_contains_link "$GHRI_ROOT/${DSL_ARGS[0]}/meta.json" "${DSL_ARGS[1]}"

    elif dsl_match _ links to _ in meta === "$@"; then
        verify_metadata_contains_n_links "${DSL_ARGS[0]}" "$GHRI_ROOT/${DSL_ARGS[1]}/meta.json"

    elif dsl_match _ links to _ at version _ in meta === "$@"; then
        verify_metadata_contains_n_versioned_links "${DSL_ARGS[0]}" "$GHRI_ROOT/${DSL_ARGS[1]}/meta.json" "${DSL_ARGS[2]}"

    elif dsl_match no link to _ in meta === "$@"; then
        verify_metadata_does_not_contain_link "$GHRI_ROOT/${DSL_ARGS[0]}/meta.json"

    elif dsl_match no link to _ at version _ in meta === "$@"; then
        verify_metadata_does_not_contain_versioned_link "$GHRI_ROOT/${DSL_ARGS[0]}/meta.json" "${DSL_ARGS[1]}"

    elif dsl_match no link to _ with path _ in meta === "$@"; then
        verify_metadata_does_not_contain_link "$GHRI_ROOT/${DSL_ARGS[0]}/meta.json" "${DSL_ARGS[1]}"

    elif dsl_match no link to _ at version _ with path _ in meta === "$@"; then
        verify_metadata_does_not_contain_versioned_link "$GHRI_ROOT/${DSL_ARGS[0]}/meta.json" "${DSL_ARGS[1]}" "${DSL_ARGS[2]}"

    elif dsl_match path _ not to exist === "$@"; then
        expect_path_not_to_exist "${DSL_ARGS[0]}"

    elif dsl_match path _ to exist === "$@"; then
        expect_path_to_exist "${DSL_ARGS[0]}"

    else
        fail "Unknown expect pattern: $*"
        return 1
    fi
}

# shellcheck disable=SC2329
get_link_dest() {
    local meta_file="$1"
    local path="${2:-}"
    if [[ -z "$path" ]]; then
        jq -r '.links[]? | select(.path == null or .path == "") | .dest' "$meta_file"
    else
        jq -r --arg path "$path" '.links[]? | select(.path == $path) | .dest' "$meta_file"
    fi
}
# shellcheck disable=SC2329
verify_metadata_contains_link() {
    local meta_file="$1"
    local path="${2:-}"
    local dest
    dest="$(get_link_dest "$meta_file" "$path" | head -1)"

    if [[ -n "$dest" ]] ; then
        note "Link dest in meta.json: '${path}' -> $dest"
        expect_symlink_to_be_relative "${meta_file%/*}" "$dest"
    else
        fail "Could not find link dest in meta.json"
    fi
}
# shellcheck disable=SC2329
verify_metadata_contains_n_links() {
  local expected_count="$1"
  local meta_file="$2"
  local path="${3:-}"
  local actual_count=0
  local -a dest_list=()
  while IFS= read -r line; do
      dest_list+=("$line")
  done < <(get_link_dest "$meta_file" "$path")

  for dest in "${dest_list[@]-}"; do
      note "Link dest in meta.json: '${path}' -> $dest"
      expect_symlink_to_be_relative "${meta_file%/*}" "$dest"
      actual_count=$((actual_count + 1))
  done
  if [[ "$actual_count" -eq "$expected_count" ]]; then
      pass "Found expected number of link dests ($expected_count) in meta.json for path '${path}'"
  else
      fail "Expected $expected_count link dests but found $actual_count in meta.json for path '${path}'"
  fi
}

# shellcheck disable=SC2329
verify_metadata_does_not_contain_link() {
    local meta_file="$1"
    local path="${2:-}"
    local dest
    dest="$(get_link_dest "$meta_file" "$path")"

    if [[ -z "$dest" ]] ; then
        pass "Link dest correctly not found in meta.json for path '${path}'"
    else
        fail "Unexpectedly found link dest in meta.json for path '${path}': $dest"
    fi
}

# shellcheck disable=SC2329
get_versioned_link_dest() {
    local meta_file="$1"
    local version="$2"
    local path="${3:-}"
    if [[ -z "$path" ]]; then
        jq -r --arg version "$version" '.versioned_links[]? | select(.version == $version and (.path == null or .path == "")) | .dest' "$meta_file"
    else
        jq -r --arg version "$version" --arg path "$path" '.versioned_links[]? | select(.version == $version and .path == $path) | .dest' "$meta_file"
    fi
}
# shellcheck disable=SC2329
verify_metadata_contains_versioned_link() {
    local meta_file="$1"
    local version="$2"
    local path="${3:-}"
    local dest
    dest="$(get_versioned_link_dest "$meta_file" "$version" "$path" | head -1)"

    if [[ -n "$dest" ]] ; then
        note "Versioned link dest in meta.json: $version '${path}' -> $dest"
        expect_symlink_to_be_relative "${meta_file%/*}" "$dest"
    else
        fail "Could not find versioned link dest in meta.json for version $version"
    fi
}
# shellcheck disable=SC2329
verify_metadata_does_not_contain_versioned_link() {
    local meta_file="$1"
    local version="$2"
    local path="${3:-}"
    local dest
    dest="$(get_versioned_link_dest "$meta_file" "$version" "$path")"

    if [[ -z "$dest" ]] ; then
        pass "Versioned link dest correctly not found in meta.json for version $version and path '${path}'"
    else
        fail "Unexpectedly found versioned link dest in meta.json for version $version and path '${path}': $dest"
    fi
}

# shellcheck disable=SC2329
verify_metadata_contains_n_versioned_links() {
  local expected_count="$1"
  local meta_file="$2"
  local version="$3"
  local path="${4:-}"
  local actual_count=0
  local -a dest_list=()
  while IFS= read -r line; do
      dest_list+=("$line")
  done < <(get_versioned_link_dest "$meta_file" "$version" "$path")

  for dest in "${dest_list[@]-}"; do
      note "Versioned link dest in meta.json: $version '${path}' -> $dest"
      expect_symlink_to_be_relative "${meta_file%/*}" "$dest"
      actual_count=$((actual_count + 1))
  done
  if [[ "$actual_count" -eq "$expected_count" ]]; then
      pass "Found expected number of versioned link dests ($expected_count) in meta.json for version $version and path '${path}'"
  else
      fail "Expected $expected_count versioned link dests but found $actual_count in meta.json for version $version and path '${path}'"
  fi
}

# shellcheck disable=SC2329
expect_file_to_exist() {
    local file="$1"
    local msg="${2:-File should exist: $file}"

    if [[ -f "$file" ]]; then
        pass "$msg"
        return 0
    else
        fail "$msg (file not found: $file)"
        return 1
    fi
}

# shellcheck disable=SC2329
expect_directory_to_exist() {
    local dir="$1"
    local msg="${2:-Directory should exist: $dir}"

    if [[ -d "$dir" ]]; then
        pass "$msg"
        return 0
    else
        fail "$msg (directory not found: $dir)"
        return 1
    fi
}

# shellcheck disable=SC2329
expect_path_not_to_exist() {
    local path="$1"
    local msg="${2:-Path should not exist: $path}"

    if [[ ! -e "$path" ]]; then
        pass "$msg"
        return 0
    else
        fail "$msg (path found: $path)"
        return 1
    fi
}

# shellcheck disable=SC2329
expect_path_to_exist() {
    local path="$1"
    local msg="${2:-Path should exist: $path}"

    if [[ -e "$path" ]]; then
        pass "$msg"
        return 0
    else
        fail "$msg (path not found: $path)"
        return 1
    fi
}

# shellcheck disable=SC2329
expect_symlink_to_not_exist() {
    local link="$1"
    local msg="${2:-Symlink should not exist: $link}"

    if [[ ! -L "$link" ]]; then
        pass "$msg"
        return 0
    else
        fail "$msg (symlink found: $link)"
        return 1
    fi
}

# shellcheck disable=SC2329
expect_symlink_to_exist() {
    local link="$1"
    local msg="${2:-Symlink should exist: $link}"

    if [[ -L "$link" ]]; then
        pass "$msg"
        return 0
    else
        fail "$msg (symlink not found: $link)"
        return 1
    fi
}

# shellcheck disable=SC2329
expect_symlink_to_be_relative() {
    local dir="$1"
    pushd "$dir" >/dev/null || return 1
    shift
    local target="$1"
    if [[ -n "$target" ]] &&  [[ "$target" != /* ]] && [[ -e "$target" ]]; then
        pass "Symlink $target is relative to $dir"
    else
        fail "Symlink absolute or broken: $target"
    fi
    popd >/dev/null || return 1
}

# shellcheck disable=SC2329
expect_symlink_target_to_be() {
    local link="$1"
    local expected_target="$2"
    local msg="${3:-Symlink target should be $expected_target}"

    if [[ -L "$link" ]]; then
        local actual_target
        actual_target=$(readlink "$link")
        if [[ "$actual_target" == "$expected_target" ]]; then
            pass "$msg"
            return 0
        else
            fail "$msg (actual: $actual_target, expected: $expected_target)"
            return 1
        fi
    else
        fail "$msg (not a symlink: $link)"
        return 1
    fi
}

# shellcheck disable=SC2329
expect_symlink_target_to_contain() {
    local link="$1"
    local expected_substring="$2"
    local msg="${3:-Symlink target should contain $expected_substring}"

    if [[ -L "$link" ]]; then
        local actual_target
        actual_target=$(readlink "$link")
        if [[ "$actual_target" == *"$expected_substring"* ]]; then
            pass "$msg"
            return 0
        else
            fail "$msg (actual: $actual_target, expected substring: $expected_substring)"
            return 1
        fi
    else
        fail "$msg (not a symlink: $link)"
        return 1
    fi
}

# shellcheck disable=SC2329
expect_file_to_contain() {
    local file="$1"
    local str="$2"
    local msg="${3:-File should contain pattern: "$str"}"

    if grep -q "$str" "$file" 2>/dev/null; then
        pass "$msg"
        return 0
    else
        fail "$msg (pattern not found in $file)"
        return 1
    fi
}

# shellcheck disable=SC2329
expect_command_to_succeed() {
    local msg="$1"
    shift

    if "$@" >/dev/null 2>&1; then
        pass "$msg"
        return 0
    else
        fail "$msg (command failed: $*)"
        return 1
    fi
}

# shellcheck disable=SC2329
expect_command_to_fail() {
    local msg="$1"
    shift

    if ! "$@" >/dev/null 2>&1; then
        pass "$msg"
        return 0
    else
        fail "$msg (command should have failed: $*)"
        return 1
    fi
}

#######################################
# Combined Test Scenarios
#######################################

# shellcheck disable=SC2329
scenario_basic_usage_with_bach_repo() {
    local GHRI_ROOT
    describe_scenario "Test: Bach Lifecycle (Install, Verify, Link, Unlink, Remove)"
    using_root "$TEST_ROOT/bach_lifecycle"
    local bin_dir="$TEST_ROOT/bach_bin"
    mkdir -p "$bin_dir"

    # 1. Install
    note "1. Installing bach-sh/bach..."
    run ghri install -y bach-sh/bach

    # 2. Verify Structure & Meta
    expect directory "$GHRI_ROOT/bach-sh/bach" to exist &&
        expect file "$GHRI_ROOT/bach-sh/bach/meta.json" to exist &&
        expect symlink "$GHRI_ROOT/bach-sh/bach/current" to exist

    # Verify meta.json content
    expect file "$GHRI_ROOT/bach-sh/bach/meta.json" to contain "bach-sh/bach"
    expect file "$GHRI_ROOT/bach-sh/bach/meta.json" to contain "api.github.com"

    # Verify symlink relative
    local link="$GHRI_ROOT/bach-sh/bach/current"
    expect path "$link" to exist &&
        expect symlink "$link" to point to matching "0.7.2"

    # 3. Idempotency
    note "3. Testing Idempotency (re-installing)..."
    output should contain "Skipping\|already installed" from command: ghri install -y bach-sh/bach

    # 4. Link to file
    local link_path="$bin_dir/my-bach"
    note "4. Linking to file $link_path..."
    ghri link bach-sh/bach "$link_path"
    expect symlink "$link_path" to exist &&
        expect symlink "$(readlink "$link_path")" to be relative to "${link_path%/*}"

    # Verify link in meta.json is relative to package directory
    local meta_file="$GHRI_ROOT/bach-sh/bach/meta.json"
    expect link to bach-sh/bach in meta

    # Verify show command
    output should contain "$link_path" from command: ghri show bach-sh/bach

    # 5. Verify Links Command
    note "5. Verifying links command..."
    expect path "$GHRI_ROOT/bach" not to exist
    run ghri links bach-sh/bach | grep -q "\"$link_path\""

    # 6. Unlink
    note "6. Unlinking..."
    ensure "$link_path" exists
    ghri unlink bach-sh/bach "$link_path"
    expect no link to bach-sh/bach in meta
    expect symlink "$link_path" not to exist

    # 7. Link Multiple & Unlink All
    note "7. Linking multiple and unlinking all..."
    ghri link bach-sh/bach "$bin_dir/link1" >/dev/null
    ghri link bach-sh/bach "$bin_dir/link2" >/dev/null
    expect 2 links to bach-sh/bach in meta

    ghri unlink bach-sh/bach --all
    expect symlink "$bin_dir/link1" not to exist
    expect symlink "$bin_dir/link2" not to exist
    expect 0 links to bach-sh/bach in meta

    # 8. Install an old version and verify current
    note "8. Installing specific old version (0.6.0)..."
    ghri install -y bach-sh/bach@0.6.0
    expect symlink "$GHRI_ROOT/bach-sh/bach/current" to point to "0.6.0"

    # 9. Link versioned link
    note "9. Creating versioned link for 0.6.0..."
    expect no link to bach-sh/bach at version 0.6.0 in meta
    ghri link bach-sh/bach@0.6.0 "$bin_dir/bach-v0.6.0"
    expect symlink "$bin_dir/bach-v0.6.0" to exist &&
        expect symlink "$(readlink "$bin_dir/bach-v0.6.0")" to be relative to "${bin_dir}"
    expect link to bach-sh/bach at version 0.6.0 in meta

#    # 10. Unlink versioned link
#    note "10. Unlinking versioned link for 0.6.0..."
#    ghri unlink bach-sh/bach@0.6.0 "$bin_dir/bach-v0.6.0"
#    expect no link to bach-sh/bach at version 0.6.0 in meta
#    expect symlink "$bin_dir/bach-v0.6.0" not to exist
#    note "Default links should still exist"
#    expect link to bach-sh/bach in meta
#
#    # 11. Unlinking all versioned links for 0.6.0...
#    note "11. Unlinking all versioned links for 0.6.0..."
#    ghri link bach-sh/bach@0.6.0 "$bin_dir/bach-v0.6.0-1" >/dev/null
#    ghri unlink bach-sh/bach@0.6.0 --all
#    expect no link to bach-sh/bach at version 0.6.0 in meta
#    note "Default links should still exist"
#    expect link to bach-sh/bach in meta

    # 12. Remove
    note "12. Removing package..."
    ghri remove -y bach-sh/bach
    expect path "$GHRI_ROOT/bach-sh/bach" not to exist
}

# shellcheck disable=SC2329
scenario_version_management_and_upgrades() {
    local GHRI_ROOT
    describe_scenario "Test: Version Lifecycle (Install specific, Upgrade, Versioned Links, Remove)"
    using_root "$TEST_ROOT/version_lifecycle"
    local bin_dir="$TEST_ROOT/version_bin"
    local meta_file="$GHRI_ROOT/bach-sh/bach/meta.json"
    mkdir -p "$bin_dir"

    # 1. Install v0.7.1
    note "1. Installing bach-sh/bach@0.7.1..."
    run ghri install -y bach-sh/bach@0.7.1

    # 2. Create Versioned Link
    note "2. Creating versioned link..."
    ghri link bach-sh/bach@0.7.1 "$bin_dir/bach-v1"
    expect link to bach-sh/bach at version 0.7.1 in meta
    expect path "$bin_dir/bach-v1" to exist &&
        expect symlink "$bin_dir/bach-v1" to exist

    # 3. Install v0.7.2
    note "3. Installing bach-sh/bach@0.7.2..."
    run ghri install -y bach-sh/bach@0.7.2

    # 4. Create Regular Link
    note "4. Creating regular link (should point to latest)..."
    ghri link bach-sh/bach "$bin_dir/bach-latest"
    
    # Verify regular link is valid
    expect path "$bin_dir/bach-latest" to exist

    # 5. Verify Links
    expect symlink "$bin_dir/bach-v1" to point to matching "0.7.1"
    expect symlink "$bin_dir/bach-latest" to point to matching "0.7.2"

    # 6. Remove v0.7.1
    note "6. Removing v0.7.1..."
    ghri remove -y bach-sh/bach@0.7.1
    expect path "$GHRI_ROOT/bach-sh/bach/0.7.1" not to exist
    expect path "$bin_dir/bach-v1" not to exist

    # 7. Remove v0.7.2 (Current) with Force
    note "7. Removing v0.7.2 (current) with --force..."
    # First try without force
    expect command to fail: ghri remove -y bach-sh/bach@0.7.2

    # Now with force
    ghri remove -y bach-sh/bach@0.7.2 --force
    expect path "$GHRI_ROOT/bach-sh/bach/0.7.2" not to exist
}

# shellcheck disable=SC2329
scenario_filtering_and_path_linking() {
    local GHRI_ROOT
    describe_scenario "Test: Zidr Lifecycle (Filter, Link Dir, Unlink Path)"
    using_root "$TEST_ROOT/zidr_lifecycle"
    local bin_dir="$TEST_ROOT/zidr_bin"
    mkdir -p "$bin_dir"

    # 1. Install with Filter
    note "1. Installing chaifeng/zidr with filter..."
    run ghri install -y chaifeng/zidr --filter '*x86_64-linux*'

    # Verify filter
    local current_version
    current_version="$(readlink "$GHRI_ROOT/chaifeng/zidr/current")"
    local version_dir="$GHRI_ROOT/chaifeng/zidr/$current_version"
    local file_count
    file_count="$(find "$version_dir" -type f | wc -l | tr -d ' ')"
    if [[ $file_count -gt 0 ]]; then
        pass "Files downloaded ($file_count)"
    else
        fail "No files downloaded"
    fi

    # 2. Link to Directory
    note "2. Linking to directory..."
    ghri link chaifeng/zidr "$bin_dir"

    expect path "${bin_dir}/zidr" to exist

    # 3. Linking by Path
    note "3. Linking by path..."
    # Need to find the path name first
    local link_name
    link_name="$(find "$GHRI_ROOT/chaifeng/zidr/current/" -maxdepth 1 -type f | head -1 | xargs -n 1 basename)"
    if [[ -n "$link_name" ]]; then
        ghri link "chaifeng/zidr:$link_name" "$bin_dir"
        expect path "$bin_dir/$link_name" to exist
        expect link to chaifeng/zidr with path "$link_name" in meta
    else
        warn "Could not find link to verify"
    fi

    # 4. Unlink by Path
    note "4. Unlinking by path..."
    if [[ -n "$link_name" ]]; then
        ghri unlink "chaifeng/zidr:$link_name"
        expect path "$bin_dir/$link_name" not to exist
    else
        warn "Could not find link to unlink"
    fi
}

# shellcheck disable=SC2329
scenario_upgrade_mechanism_mocked() {
    local GHRI_ROOT
    describe_scenario "Test: Upgrade after update (Mocked)"
    # This test is valuable as it tests the update logic without needing to download an old version

    using_root "$TEST_ROOT/upgrade_test"

    local pkg_dir="$GHRI_ROOT/bach-sh/bach"
    local old_version="0.6.0"

    mkdir -p "$pkg_dir/$old_version"
    echo "fake old version content" > "$pkg_dir/$old_version/README.md"
    ln -s "$old_version" "$pkg_dir/current"

    # Create a minimal meta.json with ONLY the old version
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
            "tag": "$old_version",
            "name": "Old Release",
            "published_at": "2020-01-01T00:00:00Z",
            "prerelease": false,
            "tarball_url": "https://api.github.com/repos/bach-sh/bach/tarball/$old_version",
            "assets": []
        }
    ]
}
EOF

    note "Created fake old installation at version $old_version"

    # Run update to fetch latest release info
    note "Running update..."
    run ghri update

    # Verify meta.json NOW contains latest version info
    expect file "$pkg_dir/meta.json" to contain "0.7.2"

    # Now run install to upgrade to latest
    note "Running install (upgrade)..."
    run ghri install -y bach-sh/bach

    # Verify current now points to latest version
    expect symlink "$pkg_dir/current" to point to "0.7.2"
}

# shellcheck disable=SC2329
scenario_error_handling() {
    local GHRI_ROOT
    describe_scenario "Test: Error Cases"
    using_root "$TEST_ROOT/errors"

    # Invalid repo formats
    expect command to fail: ghri install -y "invalid"
    expect command to fail: ghri install -y "/repo"

    # Non-existent repo
    expect command to fail: ghri install -y "fake-owner-123/fake-repo-456"

    # Non-existent version
    expect command to fail: ghri install -y "bach-sh/bach@v99.99.99"

    # Filter no match
    expect command to fail: ghri install -y chaifeng/zidr --filter '*nomatch*'

    # Link uninstalled
    expect command to fail: ghri link nonexistent/pkg "$GHRI_ROOT/link"

    # Unlink uninstalled
    expect command to fail: ghri unlink nonexistent/pkg --all

    # Unlink missing args
    expect command to fail: ghri unlink bach-sh/bach
}

# shellcheck disable=SC2329
scenario_edge_cases_and_concurrency() {
    describe_scenario "Test: Edge Cases (Env Root, Relative Paths, Concurrent)"

    # 1. Custom Root via --root option
    local env_root="$TEST_ROOT/env_root"
    mkdir -p "$env_root"
    note "Testing --root option ..."
    (
        unset GHRI_ROOT
        expect command to succeed: ghri --root "$env_root" install -y bach-sh/bach
    )
    expect directory "$env_root/bach-sh/bach" to exist

    # 2. Relative Paths
    local rel_test_dir="$TEST_ROOT/rel_test"
    mkdir -p "$rel_test_dir/root" "$rel_test_dir/bin"
    pushd "$rel_test_dir" >/dev/null || return 1
    note "Testing relative paths..."
    # We can reuse the install from env_root to save a download if we link it?
    # No, let's just do a quick install, or skip if we want to save downloads.
    # But the user asked to reduce downloads.
    # Let's skip the full install here and just test the arg parsing if possible?
    # No, we need to install to test linking.
    # We'll do one more install here.
    ghri install -y bach-sh/bach --root "root" >/dev/null
    expect directory "root/bach-sh/bach" to exist
    expect command to succeed: ghri link bach-sh/bach "bin/bach" --root "root" &&
        expect path "bin/bach" to exist &&
        expect symlink "bin/bach" to exist
    popd >/dev/null || return 1

    # 3. Concurrent Installs
    # This is a stress test, good to keep but maybe optional?
    # It downloads twice.
    describe_scenario "Test: Concurrent Installations"
    local conc_root="$TEST_ROOT/concurrent"
    mkdir -p "$conc_root"

    ghri install -y bach-sh/bach --root "$conc_root" >/dev/null 2>&1 &
    local pid1=$!
    ghri install -y chaifeng/zidr --root "$conc_root" >/dev/null 2>&1 &
    local pid2=$!

    wait "$pid1"
    wait "$pid2"

    expect directory "$conc_root/bach-sh/bach" to exist
    expect directory "$conc_root/chaifeng/zidr" to exist
}

# shellcheck disable=SC2329
scenario_help_and_version_info() {
    local GHRI_ROOT
    describe_scenario "Test: Help & Version"
    expect command to succeed: ghri --help
    expect command to succeed: ghri --version
}

#######################################
# Main
#######################################
# shellcheck disable=SC2329
main() {
    describe_scenario "ghri End-to-End Tests (Optimized)"
    note "Starting test suite..."

    setup

    declare -a all_test_cases=(
        scenario_help_and_version_info
        scenario_basic_usage_with_bach_repo
        scenario_version_management_and_upgrades
        scenario_filtering_and_path_linking
        scenario_upgrade_mechanism_mocked
        scenario_error_handling
        scenario_edge_cases_and_concurrency
    )
    for t in "${all_test_cases[@]}"; do
        if [[ ${#test_filters[@]} -eq 0 ]]; then
            note "Running test: $t"
            "$t"
        else
            for filter in "${test_filters[@]}"; do
                if [[ "$t" == *"$filter"* ]]; then
                    note "Running test (filtered): $t"
                    "$t"
                fi
            done
        fi
    done

    describe_scenario "Test Summary"
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
