# Manual Updates

Manual updates allow you to update specific packages on-demand. This is the most common workflow for maintaining individual packages or performing targeted updates.

## When to Use Manual Updates

Manual updates are ideal for:

- **Targeted fixes** - Updating a specific package that needs attention
- **Testing** - Verifying updates before automating them
- **Critical packages** - Packages requiring careful review before updating
- **One-off updates** - Packages that rarely change
- **Development** - Testing ekapkgs-update configuration

For maintaining large package sets, consider [Daemon Mode](./daemon-mode.md) instead.

## Basic Workflow

### Simple Update

Update a package to its latest version:

```bash
ekapkgs-update update mypackage
```

**What happens:**

1. **Evaluation** - Package metadata extracted from `default.nix`
2. **Version check** - Latest upstream version fetched (GitHub, GitLab, PyPI, etc.)
3. **Rewriting** - Version and hash fields updated in Nix file
4. **Build verification** - Package build attempted to verify update
5. **Success** - File is updated if build succeeds

**Output example:**

```
INFO Updating mypackage from 1.2.3 to 1.3.0
INFO Updated version: 1.2.3 -> 1.3.0
INFO Updated hash: sha256-old... -> sha256-new...
INFO Build successful
```

### Update with Commit

Automatically create a Git commit after successful update:

```bash
ekapkgs-update update mypackage --commit
```

**Commit message format:**

```
mypackage: 1.2.3 -> 1.3.0

Generated with ekapkgs-update

Co-Authored-By: ekapkgs-update <noreply@ekala-project.org>
```

You can customize commit messages by creating a `passthru.updateScript` that formats them differently.

### Update with Pull Request

Automatically create a pull request:

```bash
ekapkgs-update update mypackage \
  --create-pr \
  --upstream nixpkgs \
  --fork origin
```

**Requirements:**

- `gh` CLI tool installed and authenticated
- Git remote configured for `origin` (your fork)
- Git remote configured for `nixpkgs` (upstream)

**What happens:**

1. Package is updated
2. Git commit created
3. Branch pushed to your fork
4. Pull request created on upstream repository

**PR includes:**

- Package name and version change in title
- Changelog excerpt (if available)
- CVE check results (unless `--skip-cve`)
- Repology version comparison (unless `--skip-repology`)
- Directory size diff (unless `--skip-directory-diff`)
- Rebuild analysis (if `--analyze-rebuilds`)

## Advanced Workflows

### Specific Version Update

Update to a specific version rather than the latest:

```bash
ekapkgs-update update mypackage --version 2.5.1
```

**Use cases:**

- Pinning to a known-good version
- Rolling back a problematic update
- Testing a specific release
- Meeting dependency requirements

### Conservative Updates (SemVer)

Only update within semantic versioning constraints:

```bash
# Only patch updates (1.2.x)
ekapkgs-update update mypackage --semver patch

# Only minor updates (1.x.y)
ekapkgs-update update mypackage --semver minor

# Only major updates (latest within current major version)
ekapkgs-update update mypackage --semver major
```

**Example:**

Current version: `1.2.3`

| Strategy | Updates to | Allows |
|----------|-----------|---------|
| `latest` | `2.0.0` | Any version |
| `major` | `2.0.0` | Any version (same as latest) |
| `minor` | `1.5.0` | `1.x.y` only |
| `patch` | `1.2.8` | `1.2.x` only |

This is useful for:
- Production systems requiring stability
- Packages with breaking changes in minor/major versions
- Gradual update rollout

### Custom Version Extraction

Some packages use non-standard tag formats:

```bash
# Tags like "jq-1.6" instead of "v1.6"
ekapkgs-update update jq --version-regex 'jq-(.*)'

# Tags like "release-2.5.1"
ekapkgs-update update mypackage --version-regex 'release-(.*)'

# Tags like "2024.01.15"
ekapkgs-update update mypackage --version-regex '(.*)'
```

**Requirements:**

- POSIX extended regex syntax
- Exactly one capture group for the version
- Test regex with `--dry-run` first

See [Version Regex](../passthru-attributes/version-regex.md) for detailed documentation.

### Source-Only Updates

Update only the source hash, skipping dependency hashes:

```bash
ekapkgs-update update mypackage --src-only
```

**Skips updating:**

- `npmDepsHash` (Node.js packages)
- `cargoHash` (Rust packages)
- `vendorHash` (Go packages)
- `nugetDeps` (.NET packages)
- `composerDeps` (PHP packages)

**Use cases:**

- Dependency hash update failing
- Source changed but dependencies didn't
- Debugging which hash is problematic
- Incremental update workflow

**Follow-up:**

After `--src-only`, you can manually update dependency hashes or investigate failures.

### Update Script Override

Some packages have `passthru.updateScript`. By default, ekapkgs-update uses it if available:

```bash
# Use generic update method, ignore updateScript
ekapkgs-update update mypackage --ignore-update-script
```

**When to use:**

- Custom update script is broken
- Testing generic update method
- Update script does more than just version update
- Debugging update failures

### Force Update Skipped Packages

Override `passthru.ekapkgs-update.skip = true`:

```bash
ekapkgs-update update mypackage --force
```

**Output:**

```
WARN Package 'mypackage' has skip=true, but proceeding due to --force flag
INFO Updating mypackage from 1.2.3 to 1.3.0
```

**Use cases:**

- Temporary update needed despite skip flag
- Testing that skip flag is working
- Emergency security update

See [Skip Attribute](../passthru-attributes/skip.md) for details.

## Working with Different Files

### Custom Nix File

Update packages in a different file:

```bash
ekapkgs-update update --file ./pkgs/mypackage.nix mypackage
```

### Nested Attribute Paths

Update packages in attribute sets:

```bash
# Python package
ekapkgs-update update python3Packages.requests

# Deep nesting
ekapkgs-update update myapp.dependencies.somelib

# Flake package
ekapkgs-update update --flake .#mypackage
```

### Override Filename Detection

When `meta.position` points to the wrong file:

```bash
ekapkgs-update update mypackage --override-filename ./pkgs/actual-file.nix
```

This happens with:
- Packages defined in multiple files
- Wrapper packages
- Aliases

## Handling Different Package Types

### GitHub Releases

Most common case, works automatically:

```nix
src = fetchFromGitHub {
  owner = "example";
  repo = "mypackage";
  rev = "v${version}";
  hash = "sha256-...";
};
```

```bash
ekapkgs-update update mypackage
```

### PyPI Packages

Python packages work automatically:

```nix
src = fetchPypi {
  pname = "requests";
  version = "2.31.0";
  hash = "sha256-...";
};
```

```bash
ekapkgs-update update python3Packages.requests
```

### GitLab Projects

Similar to GitHub:

```nix
src = fetchFromGitLab {
  owner = "example";
  repo = "mypackage";
  rev = "v${version}";
  hash = "sha256-...";
};
```

```bash
ekapkgs-update update mypackage
```

### Git Tags (No Releases)

Packages using Git tags without GitHub/GitLab releases:

```nix
src = fetchgit {
  url = "https://example.com/repo.git";
  rev = "v${version}";
  hash = "sha256-...";
};
```

ekapkgs-update will fetch tags via Git and find the latest.

### Flake Packages

```bash
ekapkgs-update update --flake .#mypackage
```

**Note:** Flake packages don't support passthru attributes yet.

### mkManyVariants Packages

Update all variants or specific ones:

```bash
# Update all variants
ekapkgs-update update nodejs --all-variants

# Update only one variant
ekapkgs-update update nodejs --variant v20_x
```

See [Many Variants](../advanced/many-variants.md) for details.

## Testing and Validation

### Run Package Tests

Run `passthru.tests` before considering update successful:

```bash
ekapkgs-update update mypackage --run-passthru-tests
```

**What happens:**

1. Package is updated
2. Main package builds
3. All `passthru.tests` are built
4. If any test fails, update is rolled back

**Use cases:**

- Critical packages with comprehensive tests
- Ensuring update doesn't break functionality
- CI/CD pipelines requiring validation

### Format Updated Files

Automatically format with nixfmt:

```bash
ekapkgs-update update mypackage --format
```

**When to use:**

- Project requires consistent formatting
- CI enforces formatting checks
- Personal preference for formatted code

## Debugging Failed Updates

### Enable Debug Logging

```bash
RUST_LOG=debug ekapkgs-update update mypackage
```

**Shows:**

- Package metadata extraction
- API requests to GitHub/GitLab/PyPI
- Regex matching on tags
- Version comparison logic
- Hash computation
- Build output

### Check Update Logs

View historical failure logs:

```bash
ekapkgs-update log mypackage
```

Shows recent failed update attempts with timestamps and error messages.

### Dry Run (via run command)

Preview what would be updated without changes:

```bash
ekapkgs-update run --dry-run --file ./default.nix
```

Lists packages with available updates.

## Integration with Git Workflows

### Feature Branch Workflow

```bash
# Create feature branch
git checkout -b update-mypackage

# Update package
ekapkgs-update update mypackage --commit

# Review changes
git show
git diff master

# Push and create PR manually
git push -u origin update-mypackage
gh pr create --title "mypackage: 1.2.3 -> 1.3.0"
```

### Direct PR Creation

```bash
# One command to update and PR
ekapkgs-update update mypackage \
  --create-pr \
  --upstream nixpkgs \
  --fork origin
```

### Multiple Updates, Single PR

```bash
# Update multiple packages
ekapkgs-update update package1 --commit
ekapkgs-update update package2 --commit
ekapkgs-update update package3 --commit

# Create PR for all
git push -u origin update-branch
gh pr create --title "Update packages: package1, package2, package3"
```

## Best Practices

### Before Updating

1. **Check current state** - Ensure working directory is clean
2. **Review passthru attributes** - Understand package update configuration
3. **Read changelog** - Know what's changing upstream
4. **Check dependencies** - Consider impact on dependent packages

### During Update

1. **Start conservative** - Use `--semver patch` or `--semver minor` first
2. **Test incrementally** - Update one package at a time
3. **Watch build output** - Look for warnings and errors
4. **Enable tests** - Use `--run-passthru-tests` for critical packages

### After Updating

1. **Test functionality** - Don't just trust the build
2. **Check dependent packages** - Ensure nothing broke
3. **Review commit** - Verify changes make sense
4. **Write meaningful PR description** - Help reviewers

### Common Pitfalls

**Not checking skip flag:**

```bash
# This will fail if skip=true
ekapkgs-update update mypackage

# Use --force if intentional
ekapkgs-update update mypackage --force
```

**Assuming latest is best:**

```bash
# Latest might have breaking changes
ekapkgs-update update mypackage

# Conservative approach
ekapkgs-update update mypackage --semver minor
```

**Ignoring build failures:**

If the build fails, the update is rolled back. Investigate why:

```bash
# Check build output
RUST_LOG=debug ekapkgs-update update mypackage

# Try source-only first
ekapkgs-update update mypackage --src-only
```

**Forgetting API tokens:**

GitHub/GitLab rate limits are low without tokens:

```bash
export GITHUB_TOKEN="ghp_..."
export GITLAB_TOKEN="glpat-..."
```

## See Also

- [Daemon Mode](./daemon-mode.md) - Automated continuous updates
- [CLI Reference](../cli-reference.md) - Complete command documentation
- [Passthru Attributes](../passthru-attributes.md) - Per-package configuration
- [Troubleshooting](../troubleshooting.md) - Common problems and solutions
