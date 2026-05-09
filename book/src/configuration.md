# Configuration

ekapkgs-update offers flexible configuration through multiple layers, allowing you to customize behavior globally, per-command, or per-package.

## Configuration Hierarchy

Configuration is applied in the following order (later overrides earlier):

1. **Built-in defaults** - Sensible defaults for all options
2. **Environment variables** - API tokens, logging levels, Cachix settings
3. **Passthru attributes** - Per-package configuration in Nix files
4. **CLI arguments** - Command-line flags (highest priority)

**Example:**

```nix
# Package has semver-strategy = "patch"
passthru.ekapkgs-update.semver-strategy = "patch";
```

```bash
# CLI overrides to "latest"
ekapkgs-update update mypackage --semver latest
# Result: Uses "latest" (CLI takes precedence)
```

## Configuration Methods

### 1. Passthru Attributes (Per-Package)

Configure update behavior in the package definition itself.

**Location:** `passthru.ekapkgs-update` in your Nix file

**Available attributes:**

- `skip` - Disable automatic updates
- `semver-strategy` - Version constraint strategy
- `include-prereleases` - Allow prerelease versions
- `version-regex` - Custom tag extraction regex

**Example:**

```nix
mypackage = stdenv.mkDerivation {
  pname = "mypackage";
  version = "1.2.3";

  src = fetchFromGitHub {
    owner = "example";
    repo = "mypackage";
    rev = "v${version}";
    hash = "sha256-...";
  };

  passthru.ekapkgs-update = {
    # Only update to patch versions
    semver-strategy = "patch";

    # Don't include prereleases
    include-prereleases = false;

    # Custom tag format
    version-regex = "release-(.*)";
  };
};
```

**See:** [Passthru Attributes](./passthru-attributes.md) for complete documentation.

### 2. CLI Arguments

Override configuration on a per-command basis.

**Example:**

```bash
# Override semver strategy
ekapkgs-update update mypackage --semver minor

# Force update despite skip flag
ekapkgs-update update mypackage --force

# Use custom version regex
ekapkgs-update update mypackage --version-regex 'jq-(.*)'
```

**See:** [CLI Reference](./cli-reference.md) for all available options.

### 3. Environment Variables

Configure API tokens, logging, and runtime behavior.

**Example:**

```bash
# API tokens
export GITHUB_TOKEN="ghp_your_token_here"
export GITLAB_TOKEN="glpat-your_token_here"

# Logging
export RUST_LOG="debug"

# Cachix
export CACHIX_CACHE_NAME="my-cache"
export CACHIX_AUTH_TOKEN="your_token"
```

## Environment Variables Reference

### API Tokens

#### `GITHUB_TOKEN`

GitHub personal access token for API requests.

**Purpose:**
- Avoid rate limits (60 requests/hour → 5,000 requests/hour)
- Access private repositories
- Create pull requests

**Required scopes:**
- `public_repo` - For public repositories
- `repo` - For private repositories (includes public_repo)

**Generate:**

1. Go to GitHub Settings → Developer settings → Personal access tokens
2. Generate new token (classic)
3. Select `public_repo` scope
4. Copy token

**Usage:**

```bash
export GITHUB_TOKEN="ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
```

**Security:**

- Store in secrets manager (not in shell history or scripts)
- Use GitHub Actions secrets in CI
- Rotate regularly

#### `GITLAB_TOKEN`

GitLab personal access token for API requests.

**Purpose:**
- Avoid rate limits
- Access private projects
- Create merge requests

**Required scopes:**
- `read_api` - Read API access
- `read_repository` - Read repository data

**Generate:**

1. Go to GitLab User Settings → Access Tokens
2. Add new token
3. Select `read_api` and `read_repository` scopes
4. Copy token

**Usage:**

```bash
export GITLAB_TOKEN="glpat-xxxxxxxxxxxxxxxxxxxx"
```

### Logging

#### `RUST_LOG`

Control logging verbosity and filtering.

**Values:**
- `error` - Errors only
- `warn` - Warnings and errors
- `info` - Informational messages (default)
- `debug` - Verbose debugging output
- `trace` - Very verbose debugging output

**Usage:**

```bash
# Debug logging
export RUST_LOG=debug

# Only errors
export RUST_LOG=error

# Module-specific logging
export RUST_LOG=ekapkgs_update::rewrite=debug,info

# Multiple modules
export RUST_LOG=ekapkgs_update::rewrite=debug,ekapkgs_update::vcs_sources=trace,info
```

**Module names:**

- `ekapkgs_update::rewrite` - Nix file rewriting
- `ekapkgs_update::vcs_sources` - GitHub/GitLab/PyPI API calls
- `ekapkgs_update::package` - Package metadata extraction
- `ekapkgs_update::commands` - Command execution

**Examples:**

```bash
# Debug only rewrite operations
RUST_LOG=ekapkgs_update::rewrite=debug ekapkgs-update update mypackage

# Trace VCS API calls
RUST_LOG=ekapkgs_update::vcs_sources=trace ekapkgs-update update mypackage

# All debug output
RUST_LOG=debug ekapkgs-update run --dry-run
```

### Cachix

#### `CACHIX_CACHE_NAME`

Default Cachix cache name for pushing build outputs.

**Purpose:**
- Share build artifacts across CI runs
- Speed up builds for reviewers
- Cache successful builds

**Usage:**

```bash
export CACHIX_CACHE_NAME="my-cache"
ekapkgs-update run --file ./default.nix
```

Or override via CLI:

```bash
ekapkgs-update run --cachix-cache other-cache
```

#### `CACHIX_AUTH_TOKEN`

Cachix authentication token for pushing to cache.

**Purpose:**
- Authenticate cache uploads
- Required for writing to Cachix

**Generate:**

1. Go to https://app.cachix.org
2. Create or select cache
3. Copy authentication token

**Usage:**

```bash
export CACHIX_AUTH_TOKEN="your_token_here"
```

**Security:**

- Store in secrets manager
- Use GitHub Actions secrets
- Never commit to version control

## Configuration Examples

### Production Environment

Conservative updates with safety checks:

```bash
# Environment variables
export GITHUB_TOKEN="ghp_..."
export RUST_LOG="info"

# Command
ekapkgs-update run \
  --file ./default.nix \
  --semver minor \
  --run-passthru-tests \
  --max-rebuilds 100 \
  --concurrent-updates 2 \
  --skip-unstable
```

**Package configuration:**

```nix
critical-package = stdenv.mkDerivation {
  # ...
  passthru.ekapkgs-update = {
    semver-strategy = "patch";  # Only patch updates
    include-prereleases = false;
  };
};
```

### CI/CD Environment

Automated updates with PRs:

```bash
# Environment variables
export GITHUB_TOKEN="${{ secrets.GITHUB_TOKEN }}"
export CACHIX_AUTH_TOKEN="${{ secrets.CACHIX_AUTH_TOKEN }}"
export RUST_LOG="info"

# Command
ekapkgs-update run \
  --file ./default.nix \
  --create-pr \
  --upstream nixpkgs \
  --fork origin \
  --concurrent-updates 4 \
  --cachix-cache my-cache \
  --analyze-rebuilds \
  --max-rebuilds 200
```

### Development Environment

Verbose logging for debugging:

```bash
# Environment variables
export RUST_LOG="debug"
export GITHUB_TOKEN="ghp_..."

# Command with debug output
ekapkgs-update update mypackage \
  --semver latest \
  --dry-run
```

### Bulk Updates

Update many packages efficiently:

```bash
# Environment variables
export GITHUB_TOKEN="ghp_..."
export GITLAB_TOKEN="glpat-..."
export RUST_LOG="warn"  # Reduce noise

# Command
ekapkgs-update run \
  --file ./default.nix \
  --concurrent-updates 8 \
  --commit \
  --skip-cve \
  --skip-repology \
  --skip-directory-diff
```

## Per-Package Configuration Patterns

### Critical Packages

Require manual review, avoid automatic updates:

```nix
openssl = stdenv.mkDerivation {
  # ...
  passthru.ekapkgs-update.skip = true;
};
```

### Stable Packages

Only patch updates allowed:

```nix
nginx = stdenv.mkDerivation {
  # ...
  passthru.ekapkgs-update = {
    semver-strategy = "patch";
    include-prereleases = false;
  };
};
```

### Beta Software

Allow prereleases:

```nix
experimental-tool = stdenv.mkDerivation {
  # ...
  passthru.ekapkgs-update = {
    semver-strategy = "latest";
    include-prereleases = true;
  };
};
```

### Custom Tag Format

Handle non-standard version tags:

```nix
jq = stdenv.mkDerivation {
  # ...
  passthru.ekapkgs-update.version-regex = "jq-(.*)";
  # Matches tags like "jq-1.6" instead of "v1.6"
};
```

### Calendar Versioning

```nix
ubuntu-advantage-tools = stdenv.mkDerivation {
  # ...
  passthru.ekapkgs-update.version-regex = "(.*)";
  # Matches any tag (e.g., "2024.01.15")
};
```

## Configuration Files

ekapkgs-update does not currently support configuration files (e.g., `.ekapkgs-update.toml`).

**Alternatives:**

### Shell Script Wrapper

```bash
#!/bin/bash
# run-updates.sh

# Load environment
source ~/.ekapkgs-update.env

# Common options
OPTS=(
  --file ./default.nix
  --create-pr
  --upstream nixpkgs
  --fork origin
  --concurrent-updates 4
  --max-rebuilds 100
)

# Run
ekapkgs-update run "${OPTS[@]}" "$@"
```

### Makefile

```makefile
.PHONY: update-dry-run update-prod

# Environment file
include .env

update-dry-run:
	ekapkgs-update run \
		--file ./default.nix \
		--dry-run \
		--concurrent-updates 4

update-prod:
	ekapkgs-update run \
		--file ./default.nix \
		--create-pr \
		--upstream nixpkgs \
		--fork origin \
		--concurrent-updates 4 \
		--max-rebuilds 100 \
		--cachix-cache $(CACHIX_CACHE_NAME)
```

### Nix Flake

```nix
{
  outputs = { self, nixpkgs }: {
    apps.x86_64-linux.update = {
      type = "app";
      program = toString (nixpkgs.legacyPackages.x86_64-linux.writeShellScript "update" ''
        export GITHUB_TOKEN="$(< /run/secrets/github-token)"
        ${nixpkgs.legacyPackages.x86_64-linux.ekapkgs-update}/bin/ekapkgs-update run \
          --file ./default.nix \
          --create-pr \
          --concurrent-updates 4
      '');
    };
  };
}
```

Run with:

```bash
nix run .#update
```

## Best Practices

### API Tokens

1. **Always configure tokens** - Avoid rate limits
2. **Use secrets management** - Never hardcode
3. **Separate tokens per environment** - Dev/staging/prod
4. **Rotate regularly** - Security best practice
5. **Minimal scopes** - Only grant necessary permissions

### Logging

1. **Production: `info`** - Sufficient detail without noise
2. **CI: `info` or `warn`** - Keep logs readable
3. **Development: `debug`** - Full visibility
4. **Debugging: `trace`** - Maximum detail
5. **Module-specific** - Debug only relevant components

### Concurrency

1. **Start low** - Begin with `--concurrent-updates 1`
2. **Increase gradually** - Monitor resource usage
3. **Watch rate limits** - Especially with high concurrency
4. **CI environments** - Be conservative (2-4)
5. **Local machines** - Adjust based on cores and RAM

### Per-Package Configuration

1. **Skip critical packages** - Avoid automatic updates
2. **Conservative strategies** - Use `patch` or `minor` for stable packages
3. **Document decisions** - Comment why certain configs are used
4. **Test incrementally** - Verify config with `--dry-run` first
5. **Review regularly** - Update configs as packages mature

### Precedence Understanding

1. **CLI always wins** - Use for overrides
2. **Passthru for defaults** - Package-level preferences
3. **Environment for secrets** - Tokens and credentials
4. **Test precedence** - Use `--dry-run` with `RUST_LOG=debug`

## Troubleshooting Configuration

### Check Current Configuration

Enable debug logging to see applied configuration:

```bash
RUST_LOG=debug ekapkgs-update update mypackage --dry-run
```

Look for lines like:

```
DEBUG mypackage: Using semver strategy: Patch
DEBUG mypackage: Include prereleases: false
DEBUG mypackage: Version regex: None
DEBUG mypackage: Skip: false
```

### Verify Environment Variables

```bash
# Check if tokens are set
echo $GITHUB_TOKEN
echo $GITLAB_TOKEN

# Test token validity
curl -H "Authorization: token $GITHUB_TOKEN" https://api.github.com/user

# Check logging level
echo $RUST_LOG
```

### Validate Passthru Attributes

```bash
# Query passthru attributes directly
nix-instantiate --eval --expr '
  with import ./default.nix {};
  mypackage.passthru.ekapkgs-update or null
'
```

### Test Configuration Changes

Always test with `--dry-run` first:

```bash
# Test new configuration
ekapkgs-update run --dry-run --file ./default.nix

# Verify expected behavior
RUST_LOG=debug ekapkgs-update update mypackage --dry-run
```

## See Also

- [Passthru Attributes](./passthru-attributes.md) - Per-package configuration (EEP-0039)
- [CLI Reference](./cli-reference.md) - All command-line options
- [Installation](./installation.md) - Setting up environment variables
- [Usage Guide](./usage.md) - Practical configuration examples
