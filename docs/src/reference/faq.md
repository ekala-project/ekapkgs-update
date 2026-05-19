# Frequently Asked Questions

Common questions about ekapkgs-update.

## General

### What is ekapkgs-update?

ekapkgs-update is an automated tool for updating Nix packages. It:
- Fetches latest versions from upstream sources (GitHub, GitLab, PyPI, etc.)
- Updates Nix files with new versions and hashes
- Builds packages to verify correctness
- Creates git commits and pull requests
- Tracks failures in a database for debugging

### How is it different from nixpkgs-update?

ekapkgs-update is designed for the ekapkgs paradigm which differs from nixpkgs:
- Focus on automated, unattended updates
- Comprehensive failure tracking and debugging
- LLM-assisted failure recovery
- Multi-variant package support
- Preserved failure artifacts for retry

### Do I need to use ekapkgs to use ekapkgs-update?

No. While designed for ekapkgs, ekapkgs-update works with any Nix repository that follows standard patterns:
- `version` attribute
- Source fetchers with version references
- Standard build systems

## Installation

### Which Nix versions are supported?

ekapkgs-update requires:
- Nix 2.4 or later
- Nix with flakes enabled (recommended)
- Linux or macOS

### Can I install without flakes?

Yes, but flakes are recommended:

```bash
# With flakes (recommended)
nix profile install github:ekapkgs/ekapkgs-update

# Without flakes
nix-env -if https://github.com/ekapkgs/ekapkgs-update/archive/main.tar.gz
```

### Do I need root access?

No. ekapkgs-update runs as a regular user and only modifies:
- Your git repository
- Database file (default: `~/.cache/ekapkgs-update/db.sqlite3`)
- Temporary worktrees in `/tmp`

## Usage

### How do I update a single package?

```bash
ekapkgs-update update mypackage
```

See [Single Package Updates](../use-cases/single-package.md) for details.

### How do I update all packages?

```bash
ekapkgs-update run
```

See [Batch Updates](../use-cases/batch-updates.md) for details.

### How do I specify which version to update to?

```bash
# Latest version (default)
ekapkgs-update update mypackage

# Specific version
ekapkgs-update update mypackage --version 2.5.0

# Conservative updates (only minor/patch)
ekapkgs-update update mypackage --semver minor
```

### Can I update without creating commits?

Yes:
```bash
# Update files only
ekapkgs-update update mypackage

# Create commit
ekapkgs-update update mypackage --commit

# Create commit and PR
ekapkgs-update update mypackage --create-pr
```

## Configuration

### How do I skip a package from automated updates?

Add to the package definition:

```nix
{
  mypackage = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.ekapkgs-update = {
      skip = true;
      skip-reason = "Requires manual testing";
    };
  };
}
```

### How do I handle non-standard version tags?

Use `version-regex`:

```nix
{
  mypackage = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.ekapkgs-update = {
      version-regex = "release-(.*)";  # For tags like "release-2.5.0"
    };
  };
}
```

Or via CLI:
```bash
ekapkgs-update update mypackage --version-regex 'release-(.*)'
```

### Do I need to configure every package?

No. Most packages work with zero configuration if they use:
- `fetchFromGitHub`, `fetchFromGitLab`, or `fetchPypi`
- Standard version tags (e.g., `v1.2.3`, `1.2.3`)
- `version` attribute with source reference

## Features

### Does it work with flakes?

Yes:
```bash
ekapkgs-update update --flake my-package
```

### Can it run tests?

Yes:
```bash
ekapkgs-update update mypackage --run-passthru-tests
```

Runs all derivations in `passthru.tests`.

### Does it support Cachix?

Yes:
```bash
export CACHIX_AUTH_TOKEN="..."
ekapkgs-update run --cachix-cache my-cache
```

### Can it analyze rebuild impact?

Yes:
```bash
ekapkgs-update run --analyze-rebuilds --max-rebuilds 100
```

### Does it work with private repositories?

Yes, set `GITHUB_TOKEN` environment variable with repo scope.

## Troubleshooting

### Update fails with "hash mismatch"

This usually means upstream changed the release tarball. ekapkgs-update automatically fetches the new hash. If it still fails:

```bash
# Retry
ekapkgs-update update mypackage

# Or try specific version
ekapkgs-update update mypackage --version 2.5.0
```

### Update fails with "no compatible version found"

Try a different semver strategy:

```bash
# Allow major version updates
ekapkgs-update update mypackage --semver latest

# Or specify explicit version
ekapkgs-update update mypackage --version 2.6.0
```

### Build fails after update

Preserve the failure for debugging:

```bash
# Run with preservation
ekapkgs-update update mypackage --preserve-failures

# Inspect failure
ekapkgs-update inspect mypackage
ekapkgs-update log mypackage

# View worktree
ekapkgs-update worktrees show mypackage

# Fix and retry
ekapkgs-update retry mypackage
```

### Rate limiting issues

Set GitHub token for higher limits:

```bash
export GITHUB_TOKEN="ghp_xxxxxxxxxxxxx"
# 5000 requests/hour instead of 60
```

### How do I debug a failed update?

See [Debugging Guide](../use-cases/debugging.md).

## Database

### Where is the database stored?

Default: `~/.cache/ekapkgs-update/db.sqlite3`

Override:
```bash
ekapkgs-update run --database /custom/path/db.sqlite3
```

### Can I inspect the database?

Yes:
```bash
sqlite3 ~/.cache/ekapkgs-update/db.sqlite3

# Or use commands
ekapkgs-update status
ekapkgs-update query
```

### Can I share the database across machines?

Yes, but use a shared filesystem or database server. SQLite supports concurrent reads but serialize writes.

### How do I clean old data?

```bash
# Clean old worktrees
ekapkgs-update worktrees clean --older-than 7

# Manual database cleanup (SQL)
sqlite3 ~/.cache/ekapkgs-update/db.sqlite3 \
  "DELETE FROM update_attempts WHERE timestamp < datetime('now', '-30 days')"
```

## Performance

### How many packages can it update concurrently?

Default: CPU cores / 4

Adjust:
```bash
ekapkgs-update run --concurrent-updates 16
```

### How long does a typical update take?

Varies by package:
- Simple package: 30-60 seconds
- Complex package: 2-5 minutes
- Package with tests: 5-15 minutes

Batch updates depend on concurrency and package count.

### Can I speed it up?

Yes:
```bash
# Skip optional checks
ekapkgs-update run \
  --skip-cve \
  --skip-repology \
  --skip-directory-diff \
  --concurrent-updates 16

# Use Cachix to avoid rebuilds
ekapkgs-update run --cachix-cache my-cache
```

## Integration

### Can I use it in CI/CD?

Yes! See [CI/CD Integration](../use-cases/ci-cd.md).

Example:
```yaml
# .github/workflows/update.yml
- name: Update packages
  env:
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
  run: ekapkgs-update run --dry-run
```

### Does it work with NixOS?

Yes, via the NixOS module:

```nix
{
  services.ekapkgs-update = {
    enable = true;
    schedule = "daily";
  };
}
```

See [NixOS Module](../nixos-module.md).

### Can I use it with systemd?

Yes! See [Systemd Service](../systemd.md).

### Is there a web UI?

Yes! See [Web Dashboard](../web-dashboard.md).

```bash
ekapkgs-update-web --port 3000
# Open http://localhost:3000
```

## LLM Integration

### How does LLM integration work?

Export failure context:
```bash
ekapkgs-update export mypackage --format markdown > context.md
```

Provide `context.md` to ChatGPT/Claude, get fix.patch, then:
```bash
ekapkgs-update apply mypackage --patch fix.patch --resume
```

See [LLM Integration](../cli/export-apply.md).

### Which LLMs work best?

Most effective:
- GPT-4 / GPT-4 Turbo
- Claude 3 Opus / Sonnet
- Claude Code

Less effective but usable:
- GPT-3.5
- Claude 3 Haiku
- Open-source models (Llama, Mistral)

### Do I need an API key?

No. The LLM integration is manual:
1. Export context
2. Paste to LLM (web interface)
3. Copy fix.patch from LLM
4. Apply patch

For automated integration, you'd need to use LLM APIs yourself.

## Contributing

### How can I contribute?

- Report issues on GitHub
- Submit pull requests
- Improve documentation
- Add support for new VCS sources
- Share your use cases

### Can I add support for a new source type?

Yes! See the architecture docs. Main areas:
- `src/vcs_sources/` - Add new source type
- Add tests
- Update documentation

## Support

### Where can I get help?

- GitHub Issues: Bug reports and feature requests
- Discussions: Questions and community support
- Documentation: This site

### How do I report a bug?

GitHub Issues with:
- ekapkgs-update version
- Command that failed
- Error message
- Minimal reproduction

### Is there a community?

GitHub Discussions for:
- Questions
- Show and tell
- Ideas
- General discussion

## License

### What license is ekapkgs-update under?

MIT License. See LICENSE file.

### Can I use it commercially?

Yes, MIT license allows commercial use.

## Roadmap

### What features are planned?

Check GitHub Issues and project roadmap for:
- Planned features
- In-progress work
- Future ideas

### Can I request features?

Yes! Create a GitHub Issue with:
- Use case description
- Why it's useful
- Proposed implementation (optional)
