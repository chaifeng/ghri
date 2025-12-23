# ghri - GitHub Release Installer

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)

**ghri** is a command-line tool to download and install binaries from GitHub Releases. It detects your system architecture, downloads the right asset, and manages multiple versions.

## ✨ Features

- 🚀 **One-command install** - Install binaries directly from GitHub Releases
- 🔄 **Version management** - Install, switch, and manage multiple versions
- 🔗 **Symlink management** - Create symlinks to any location
- 🎯 **Smart matching** - Auto-detect system architecture and select the right asset
- 🔒 **Private repo support** - Access private repos with GitHub Token
- 📦 **No root needed** - Install to user directory by default

## 📥 Installation

### One-line Install (Recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/chaifeng/ghri/main/install.sh | sh
```

Or specify a custom bin directory:

```bash
curl -fsSL https://raw.githubusercontent.com/chaifeng/ghri/main/install.sh | sh -s -- /custom/bin/path
```

### Manual Install

1. Download the archive for your platform from [Releases](https://github.com/chaifeng/ghri/releases)
2. Extract to any directory
3. Add the `ghri` binary to your PATH

### Supported Platforms

| OS | Architecture | Filename |
|----|--------------|----------|
| macOS | Apple Silicon (M1/M2) | `ghri-*-aarch64-apple-darwin.tar.gz` |
| macOS | Intel | `ghri-*-x86_64-apple-darwin.tar.gz` |
| Linux | ARM64 | `ghri-*-aarch64-unknown-linux-gnu.tar.gz` |
| Linux | x86_64 | `ghri-*-x86_64-unknown-linux-gnu.tar.gz` |
| Windows | ARM64 | `ghri-*-aarch64-pc-windows-msvc.zip` |
| Windows | x86_64 | `ghri-*-x86_64-pc-windows-msvc.zip` |

## 🚀 Quick Start

### Install a Package

```bash
# Install latest stable version
ghri install chaifeng/zidr

# Install a specific version
ghri install chaifeng/zidr@v0.1.0

# Install latest version (including pre-release)
ghri install chaifeng/zidr --pre

# Install a shell script library
ghri install bach-sh/bach
```

### List Installed Packages

```bash
ghri list
```

### Show Package Details

```bash
ghri show chaifeng/zidr
```

### Update Package Info

```bash
# Update release info for all installed packages
ghri update
```

## 📖 Commands

### install - Install a Package

Install a package from a GitHub repository. ghri auto-detects your system and downloads the matching asset.

Before installing, ghri shows you what files will be downloaded and what changes will be made to your system. You need to confirm before proceeding.

```bash
ghri install <OWNER/REPO[@VERSION]> [OPTIONS]
```

**Arguments:**
- `OWNER/REPO` - GitHub repository in `owner/repo` format
- `@VERSION` - Optional. Specify a version (e.g., `@v1.0.0`)

**Options:**
- `-f, --filter <PATTERN>` - Filter assets by glob pattern (can use multiple times)
- `--pre` - Allow installing pre-release versions
- `-y, --yes` - Skip confirmation prompt
- `--api-url <URL>` - Custom GitHub API URL (for GitHub Enterprise)
- `-r, --root <PATH>` - Custom install root directory

**Examples:**

```bash
# Install zidr (a binary tool)
ghri install chaifeng/zidr

# Install a specific version
ghri install chaifeng/zidr@v0.1.0

# Install bach (Bach Unit Testing Framework)
ghri install bach-sh/bach

# Install a pre-release version
ghri install chaifeng/zidr --pre

# Skip confirmation prompt (useful for scripts)
ghri install chaifeng/zidr -y

# Install to custom directory
ghri install bach-sh/bach --root ~/src/my-project/vendor # Install bach-sh/bach to your project's vendor directory
ghri install chaifeng/zidr --root ~/my-apps
```

### list - List Installed Packages

```bash
ghri list
```

Example output:
```
bach-sh/bach    v1.0.0 (current)
chaifeng/zidr   v0.1.0 (current)
```

### show - Show Package Details

```bash
ghri show chaifeng/zidr
```

Shows detailed info about a package:
- Current version
- All installed versions
- Symlink rules
- Metadata

### update - Update Release Info

Fetch latest release info from GitHub API for installed packages.

```bash
ghri update [OWNER/REPO]...
```

**Arguments:**
- `OWNER/REPO` - Optional. Packages to update. If not specified, updates all installed packages.

**Examples:**

```bash
# Update all installed packages
ghri update

# Update specific packages only
ghri update chaifeng/zidr bach-sh/bach
```

This does not upgrade packages. It only updates the local release info cache. To upgrade, run `ghri install` again.

### link - Create Symlinks

Link files from an installed package to a destination path.

```bash
ghri link <OWNER/REPO[@VERSION][:PATH]> <DEST>
```

**Arguments:**
- `OWNER/REPO` - Package name
- `@VERSION` - Optional. Specify a version (default: current)
- `:PATH` - Optional. File path inside the package
- `DEST` - Destination path for the symlink

**Version Behavior:**
- **Without version** (`owner/repo:file`) - Link follows the `current` symlink. When you install a new version, the link auto-updates to the new version.
- **With version** (`owner/repo@v1.0.0:file`) - Link points to that specific version. It stays unchanged when you install new versions.

**Link Uniqueness:**
Each destination path can only have one link type. If you create a versioned link to a destination that already has a default link, the default link record is removed (and vice versa).

**Examples:**

```bash
# Link zidr binary to ~/.local/bin/zidr (auto-updates on new install)
ghri link chaifeng/zidr:zidr ~/.local/bin/zidr

# Link bach.sh to your project's test directory (auto-updates)
ghri link bach-sh/bach:bach.sh ~/my-project/test/bach.sh

# Install and link bach.sh to your project's test directory
ghri --root ~/src/my-project/vendor link bach-sh/bach:bach.sh ~/src/my-project/tests/bach.sh


# Link a specific version (stays at v0.1.0 forever)
ghri link chaifeng/zidr@v0.1.0:zidr ~/.local/bin/zidr
```

### unlink - Remove Symlinks

Remove symlinks and link rules.

```bash
ghri unlink <OWNER/REPO[:PATH]> [DEST] [OPTIONS]
```

**Options:**
- `-a, --all` - Remove all link rules for the package

**Examples:**

```bash
# Remove a specific link
ghri unlink chaifeng/zidr ~/.local/bin/zidr

# Remove all links for a package
ghri unlink bach-sh/bach --all
```

### links - Show Link Rules

Show all symlink rules for a package.

```bash
ghri links chaifeng/zidr
```

### remove - Remove a Package

Remove an installed package or a specific version.

Before removing, ghri shows you what files and directories will be deleted. You need to confirm before proceeding.

```bash
ghri remove <OWNER/REPO[@VERSION]> [OPTIONS]
```

**Options:**
- `-f, --force` - Force removal of current version
- `-y, --yes` - Skip confirmation prompt

**Examples:**

```bash
# Remove entire package
ghri remove chaifeng/zidr

# Remove a specific version
ghri remove chaifeng/zidr@v0.1.0

# Force remove current version
ghri remove chaifeng/zidr@v0.1.0 --force

# Skip confirmation (useful for scripts)
ghri remove bach-sh/bach -y
```

## ⚙️ Configuration

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `GHRI_ROOT` | Install root directory | `~/.ghri` |
| `GHRI_API_URL` | GitHub API URL | `https://api.github.com` |
| `GITHUB_TOKEN` | GitHub access token | - |

### GitHub Token

Set `GITHUB_TOKEN` to:
- Access private repositories
- Increase API rate limit (from 60/hour to 5000/hour)

```bash
export GITHUB_TOKEN=ghp_xxxxxxxxxxxx
```

### Directory Structure

ghri stores installed packages in this structure:

```
~/.ghri/
├── owner1/
│   └── repo1/
│       ├── meta.json          # Metadata file
│       ├── current -> v1.0.0  # Current version symlink
│       ├── v1.0.0/            # Version directory
│       │   └── ...            # Extracted files
│       └── v0.9.0/
│           └── ...
└── owner2/
    └── repo2/
        └── ...
```

## 🔧 Advanced Usage

### Using Filters

When a release has multiple assets, ghri tries to auto-match your system. If auto-match fails or you want a specific build, use `--filter`:

```bash
# Select musl static build
ghri install chaifeng/zidr --filter "*musl*"

# Combine multiple filters
ghri install chaifeng/zidr --filter "*linux*" --filter "*x86_64*"
```

Filters are saved to metadata. They apply automatically on future updates.

### GitHub Enterprise Support

For GitHub Enterprise servers, use `--api-url`:

```bash
ghri install myorg/myrepo --api-url https://github.mycompany.com/api/v3
```

Or set the environment variable:

```bash
export GHRI_API_URL=https://github.mycompany.com/api/v3
ghri install myorg/myrepo
```

### Custom Install Directory

```bash
# Use once
ghri install chaifeng/zidr --root ~/my-apps

# Or set environment variable
export GHRI_ROOT=~/my-apps
ghri install chaifeng/zidr
```

### Switch Versions

ghri supports multiple versions. You can switch between them:

```bash
# Install multiple versions
ghri install chaifeng/zidr@v0.1.0
ghri install chaifeng/zidr@v0.2.0

# Switch back to old version (re-install updates the current link)
ghri install chaifeng/zidr@v0.1.0
```

## 👨‍💻 Development

### Pre-commit Hooks

This project uses [pre-commit](https://pre-commit.com/) to ensure code quality. The hooks verify:
- ✅ Code compiles without warnings
- ✅ Clippy lints pass
- ✅ Code formatting is correct
- ✅ Tests pass

#### Setup

1. Install pre-commit:
   ```bash
   # macOS
   brew install pre-commit
   
   # or using pip
   pip install pre-commit
   ```

2. Install the git hooks:
   ```bash
   pre-commit install
   ```

3. (Optional) Run hooks manually on all files:
   ```bash
   pre-commit run --all-files
   ```

Now, every time you `git commit`, the hooks will run automatically. If any check fails, the commit will be blocked until you fix the issues.

#### What Each Hook Does

- **cargo check** - Compiles with `-D warnings` to treat all warnings as errors
- **cargo clippy** - Runs Rust linter with `-D warnings`
- **cargo fmt** - Checks code formatting
- **cargo test** - Runs all tests

#### Skip Specific Hooks

To temporarily skip specific hooks during commit:

```bash
# Skip only cargo-clippy and cargo-fmt
SKIP=cargo-clippy,cargo-fmt git commit -m "your message"

# Skip only cargo-test (useful for quick WIP commits)
SKIP=cargo-test git commit -m "WIP: work in progress"

# Skip all pre-commit hooks (not recommended)
git commit --no-verify -m "your message"
```

### Manual Verification

If you prefer not to use pre-commit, ensure your code passes these checks before committing:

```bash
# Check compilation without warnings
RUSTFLAGS="-D warnings" cargo check --all-targets --all-features

# Run clippy
cargo clippy --all-targets --all-features -- -D warnings

# Check formatting
cargo fmt --all -- --check

# Run tests
cargo test --all-features
```

## 🤝 Contributing

Contributions are welcome! Feel free to open issues or pull requests.

## 📄 License

This project is licensed under [GNU General Public License v3.0](LICENSE).

---

**ghri** - Install GitHub Releases with ease 🎉
