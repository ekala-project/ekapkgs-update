# CLI Reference

Complete command-line interface documentation for ekapkgs-update.

## Synopsis

```bash
ekapkgs-update [OPTIONS] <COMMAND>
```

## Global Options

### `--color <WHEN>`

Control colored output.

**Values:**
- `auto` - Color when stdout/stderr is a terminal (default)
- `always` - Always emit color, even when not a terminal
- `never` - Never emit color

**Example:**

```bash
ekapkgs-update --color never update mypackage
```

### `-h, --help`

Print help information.

```bash
ekapkgs-update --help
ekapkgs-update update --help
```

## Commands

### `update`

Update a specific package in a Nix file.

**Usage:**

```bash
ekapkgs-update update [OPTIONS] <ATTR_PATH>
```

**Arguments:**

- `<ATTR_PATH>` - Attribute path of the package to update (e.g., `mypackage`, `python3Packages.requests`)

**Options:**

#### File and Evaluation

**`-f, --file <FILE>`**

Nix file to update.

- **Default:** `default.nix`
- **Example:** `--file ./pkgs/mypackage.nix`

**`--system <SYSTEM>`**

System to use for evaluation.

- **Example:** `--system x86_64-linux`, `--system aarch64-darwin`
- **Default:** Current system

**`--flake`**

Enable flake mode: update a package exposed by a flake.

- **Example:** `ekapkgs-update update --flake .#mypackage`

**`--flake-output <FLAKE_OUTPUT>`**

Flake output prefix (e.g., `packages.x86_64-linux`).

- Auto-detected if not specified
- **Example:** `--flake-output packages.aarch64-darwin`

#### Version Selection

**`--semver <STRATEGY>`**

Version selection strategy for semantic versioning.

- **Values:** `latest`, `major`, `minor`, `patch`
- **Default:** `latest`
- **Example:** `--semver minor` (only update to latest 1.x.y version)

See [Semver Strategy](./passthru-attributes/semver-strategy.md) for details.

**`--version <VERSION>`**

Explicit version to update to (overrides `--semver`).

- Can be a specific version like `2.5.1`
- Can be a tag name like `v2.5.1`
- **Example:** `--version 3.0.0`

**`--version-regex <REGEX>`**

Custom regex to extract version from tags.

- Must use POSIX extended regex syntax
- Must have exactly one capture group
- **Example:** `--version-regex 'jq-(.*)'` for tags like `jq-1.6`

See [Version Regex](./passthru-attributes/version-regex.md) for details.

#### Update Behavior

**`--force`**

Force update even if package has `passthru.ekapkgs-update.skip = true`.

```bash
ekapkgs-update update mypackage --force
```

**`--ignore-update-script`**

Ignore `passthru.updateScript` and use generic update method.

```bash
ekapkgs-update update mypackage --ignore-update-script
```

**`--src-only`**

Only update source hash, skip dependency hashes.

- Skips: `npmDeps`, `nugetDeps`, `composerDeps`, etc.
- Useful when dependency updates fail

```bash
ekapkgs-update update mypackage --src-only
```

**`--override-filename <FILENAME>`**

Override the filename to update.

- Useful when `meta.position` points to the wrong file
- **Example:** `--override-filename ./pkgs/custom.nix`

#### Git and PR Options

**`--commit`**

Create a git commit after successful update.

```bash
ekapkgs-update update mypackage --commit
```

**`--create-pr`**

Create a pull request after successful update (implies `--commit`).

- Requires `gh` CLI tool
- Automatically pushes to fork and creates PR

```bash
ekapkgs-update update mypackage --create-pr
```

**`--upstream <REMOTE>`**

Upstream git remote for pull requests.

- Inferred if left unset
- **Example:** `--upstream nixpkgs`
- Used to determine PR target

**`--fork <REMOTE>`**

Remote repository to push branches for PRs.

- **Default:** `origin`
- **Example:** `--fork my-fork`

**`--skip-directory-diff`**

Skip directory diff comparison in PR body.

- Faster for large packages
- Omits size change information

#### Testing

**`--run-passthru-tests`**

Run `passthru.tests` if available before considering update successful.

```bash
ekapkgs-update update mypackage --run-passthru-tests
```

#### Variants

**`--variant <VARIANT>`**

For `mkManyVariants` packages: update only a specific variant.

- **Example:** `--variant v1_2`, `--variant v0_20`

```bash
ekapkgs-update update nodejs --variant v18_x
```

**`--all-variants`**

For `mkManyVariants` packages: explicitly update all variants (default behavior).

#### Formatting

**`--format`**

Format updated files using `nixfmt`.

```bash
ekapkgs-update update mypackage --format
```

**Examples:**

```bash
# Basic update
ekapkgs-update update mypackage

# Update with commit
ekapkgs-update update mypackage --commit

# Update and create PR
ekapkgs-update update mypackage --create-pr --upstream nixpkgs

# Update to specific version
ekapkgs-update update mypackage --version 2.5.1

# Conservative update (only patch versions)
ekapkgs-update update mypackage --semver patch

# Force update a skipped package
ekapkgs-update update mypackage --force

# Update flake package
ekapkgs-update update --flake .#mypackage
```

---

### `run`

Run daemon mode to continuously check and update packages.

**Usage:**

```bash
ekapkgs-update run [OPTIONS]
```

**Options:**

#### File and Evaluation

**`-f, --file <FILE>`**

Nix file to evaluate.

- **Default:** `default.nix`
- **Example:** `--file ./pkgs/default.nix`

**`-d, --database <DATABASE>`**

Path to SQLite database for tracking updates.

- **Default:** `~/.cache/ekapkgs-update/updates.db`
- **Example:** `--database ./my-updates.db`

The database tracks:
- Update history
- Last checked timestamps
- Failed update logs

#### Git and PR Options

**`--upstream <REMOTE>`**

Upstream git remote for pull requests.

- Inferred if left unset
- **Example:** `--upstream nixpkgs`

**`--fork <REMOTE>`**

Remote repository to push branches.

- **Default:** `origin`
- **Example:** `--fork my-fork`

#### Behavior Control

**`--dry-run`**

Check for updates without rewriting, building, committing, or creating PRs.

- Useful for testing
- Shows which packages have updates available

```bash
ekapkgs-update run --dry-run
```

**`--concurrent-updates <N>`**

Maximum number of concurrent package updates.

- **Default:** CPU cores / 4
- **Example:** `--concurrent-updates 8`

```bash
ekapkgs-update run --concurrent-updates 4
```

**`--skip-unstable`**

Skip packages with `unstable` in their version.

- Avoids updating packages on rolling release channels

```bash
ekapkgs-update run --skip-unstable
```

#### Testing

**`--run-passthru-tests`**

Run `passthru.tests` if available before considering update successful.

#### Rebuild Control

**`--analyze-rebuilds`**

Analyze and report rebuild counts for each update.

- Shows impact of each update
- Useful for large package sets

**`--max-rebuilds <N>`**

Skip updates that would cause more than N rebuilds.

- Helps avoid massive rebuilds
- **Example:** `--max-rebuilds 100`

```bash
ekapkgs-update run --max-rebuilds 50
```

#### PR Enhancements

**`--skip-cve`**

Skip CVE vulnerability checking.

- Faster updates
- Omits security information from PRs

**`--skip-repology`**

Skip Repology cross-distribution version checking.

- Faster updates
- Omits Repology information from PRs

**`--skip-directory-diff`**

Skip directory diff comparison in PR body.

#### Cachix Integration

**`--skip-cachix`**

Skip pushing build outputs to Cachix.

**`--cachix-cache <NAME>`**

Cachix cache name to push successful builds to.

- Falls back to `CACHIX_CACHE_NAME` environment variable
- Requires `CACHIX_AUTH_TOKEN` to be set
- **Example:** `--cachix-cache my-cache`

```bash
export CACHIX_AUTH_TOKEN="..."
ekapkgs-update run --cachix-cache my-cache
```

**Examples:**

```bash
# Basic daemon mode
ekapkgs-update run

# Dry run to see what would be updated
ekapkgs-update run --dry-run

# With automatic PRs
ekapkgs-update run --create-pr --upstream nixpkgs --fork origin

# Limit concurrent updates
ekapkgs-update run --concurrent-updates 4

# Skip unstable packages and limit rebuilds
ekapkgs-update run --skip-unstable --max-rebuilds 100

# Full automation with Cachix
ekapkgs-update run \
  --create-pr \
  --upstream nixpkgs \
  --concurrent-updates 8 \
  --cachix-cache my-cache \
  --analyze-rebuilds
```

---

### `migrate`

Migrate a package from nixpkgs to ekapkgs paradigms.

**Usage:**

```bash
ekapkgs-update migrate [OPTIONS] <TARGET>
```

**Arguments:**

- `<TARGET>` - Attribute path or file path to migrate

**Options:**

**`-f, --file <FILE>`**

Nix file to evaluate (for attr paths).

- **Default:** `default.nix`

**Example:**

```bash
# Migrate by attribute path
ekapkgs-update migrate --file ./default.nix mypackage

# Migrate a file directly
ekapkgs-update migrate ./pkgs/mypackage/default.nix
```

---

### `log`

Show update failure logs for a package.

**Usage:**

```bash
ekapkgs-update log <ATTR_PATH>
```

**Arguments:**

- `<ATTR_PATH>` - Attribute path of the package

**Example:**

```bash
ekapkgs-update log mypackage
```

**Output:**

Shows recent failed update attempts with:
- Timestamp
- Error message
- Debug information

---

### `prune-maintainers`

Prune maintainers from all `.nix` files in a directory.

**Usage:**

```bash
ekapkgs-update prune-maintainers [OPTIONS]
```

**Note:** This is a specialized command for bulk operations.

---

## Environment Variables

### API Tokens

**`GITHUB_TOKEN`**

GitHub API token for accessing GitHub releases and repositories.

- **Required for:** Avoiding rate limits, accessing private repositories
- **Scopes needed:** `public_repo` (or `repo` for private repos)

```bash
export GITHUB_TOKEN="ghp_your_token_here"
```

**`GITLAB_TOKEN`**

GitLab API token for accessing GitLab repositories.

- **Required for:** Avoiding rate limits, accessing private projects
- **Scopes needed:** `read_api`, `read_repository`

```bash
export GITLAB_TOKEN="glpat-your_token_here"
```

### Logging

**`RUST_LOG`**

Control logging verbosity.

- **Values:** `error`, `warn`, `info`, `debug`, `trace`
- **Default:** `info`

```bash
# Debug logging (very verbose)
export RUST_LOG=debug

# Only warnings and errors
export RUST_LOG=warn
```

**Module-specific logging:**

```bash
# Debug only specific modules
export RUST_LOG=ekapkgs_update::rewrite=debug,info
```

### Cachix

**`CACHIX_CACHE_NAME`**

Default Cachix cache name.

- Falls back to this if `--cachix-cache` is not specified

```bash
export CACHIX_CACHE_NAME="my-cache"
```

**`CACHIX_AUTH_TOKEN`**

Cachix authentication token.

- Required for pushing to Cachix
- Get from https://app.cachix.org

```bash
export CACHIX_AUTH_TOKEN="your-auth-token"
```

## Exit Codes

- `0` - Success
- `1` - General error
- `2` - Invalid arguments
- Other non-zero values indicate errors

## See Also

- [Quick Start](./quick-start.md) - Basic usage examples
- [Passthru Attributes](./passthru-attributes.md) - Per-package configuration
- [Usage Guide](./usage.md) - Detailed usage patterns
- [Configuration](./configuration.md) - Configuration options
