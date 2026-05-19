# Security Considerations

ekapkgs-update automates package updates and interacts with external services (GitHub, PyPI, OSV, Repology, Cachix). This document covers security practices and considerations.

## Overview

Security concerns fall into these categories:

1. **Token and Credential Management**
2. **Network Security and Data Privacy**
3. **Code Execution and Sandboxing**
4. **File System Permissions**
5. **Dependency and Supply Chain Security**

## Token and Credential Management

### GitHub/GitLab Tokens

ekapkgs-update uses VCS tokens to create and manage pull requests. These are sensitive.

#### Safe Storage

Store tokens in environment variables or secure credential stores:

**Option 1: Environment Variables (Simple)**

```bash
export GITHUB_TOKEN="ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
export GITLAB_TOKEN="glpat-xxxxxxxxxxxxxxxxxxxxxxxxxxxx"

ekapkgs-update run --config config.toml
```

**Option 2: Credential Manager (Recommended)**

Use your OS credential store:

```bash
# macOS (Keychain)
security add-generic-password -s ekapkgs-update -a github -w "$GITHUB_TOKEN"
ekapkgs-update run --config config.toml  # Reads from Keychain

# Linux (secretsmanager, pass, or 1password)
pass show ekapkgs-update/github > ~/.github-token
export GITHUB_TOKEN="$(cat ~/.github-token)"

# Windows (Credential Manager)
cmdkey /add:ekapkgs-update /user:github /pass:$GITHUB_TOKEN
```

**Option 3: .env File (Local Testing Only)**

```bash
# .env (never commit to git)
GITHUB_TOKEN=ghp_xxx
GITLAB_TOKEN=glpat-xxx

# Load before running
export $(cat .env | xargs)
ekapkgs-update run
```

**Never:**
```bash
ekapkgs-update run --token "ghp_xxx"           # Visible in process list
echo "GITHUB_TOKEN=ghp_xxx" >> ~/.bashrc       # Stored in shell history
git commit .env                                # Committed to version control
```

#### Token Scope

Request minimal required permissions:

**GitHub:**
```
Permissions:
  - Contents: read/write (required for PR creation)
  - Pull requests: read/write (required for PR management)
  - Workflows: read (optional, for CI status)

Exclude:
  - Admin access
  - Secrets access
  - Organization settings
```

**GitLab:**
```
Scopes:
  - api (required for PR creation)
  - read_user (optional)

Exclude:
  - sudo
  - read_repository_variable (can read CI secrets)
```

#### Token Rotation

Rotate tokens regularly:

```bash
# 1. Generate new token in GitHub/GitLab UI
# 2. Update credential store
security update-generic-password -s ekapkgs-update -a github -w "$NEW_TOKEN"

# 3. Revoke old token in UI
# 4. Verify new token works
GITHUB_TOKEN="$NEW_TOKEN" ekapkgs-update query python3Packages.requests
```

### Cachix Authentication

Cachix push requires authentication:

```bash
export CACHIX_AUTH_TOKEN="eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
export CACHIX_CACHE_NAME="my-cache"

ekapkgs-update run --cachix-cache my-cache
```

**Security:**
- Token grants write access to cache (signing power)
- Should be restricted to CI/automation accounts
- Rotate if compromised
- Keep separate from GitHub token

## Network Security

### API Communication

ekapkgs-update communicates with:

| Service | Protocol | Data | Risk |
|---------|----------|------|------|
| GitHub API | HTTPS | Public repos, PR content | Low |
| PyPI | HTTPS | Package versions, metadata | Low |
| OSV Database | HTTPS | Vulnerability data | Low |
| Repology API | HTTPS | Package versions | Low |
| Cachix | HTTPS | Build artifacts | Medium |

All communication uses HTTPS (TLS 1.2+).

### Proxy Support

For environments behind corporate proxies:

```bash
export HTTP_PROXY="http://proxy.example.com:3128"
export HTTPS_PROXY="https://proxy.example.com:3128"
export NO_PROXY="localhost,127.0.0.1"

ekapkgs-update run --config config.toml
```

### Certificate Verification

Disable only for testing with self-signed certs:

```bash
# NOT recommended for production
export REQWEST_CLIENT_INSECURE=1
ekapkgs-update run
```

Prefer fixing certificate issues:

```bash
# Add custom CA to system store
sudo cp /path/to/ca.crt /etc/ssl/certs/
sudo update-ca-certificates  # On Linux

# Then run normally
ekapkgs-update run
```

## Code Execution and Sandboxing

### Nix Evaluation Safety

ekapkgs-update evaluates Nix expressions, which can execute code. This is inherent to Nix and cannot be sandboxed.

**Mitigations:**

1. **Review package expressions** before updating
2. **Audit unfamiliar packages** for malicious code
3. **Use Nix build sandboxing** (enabled by default):

```nix
# /etc/nix/nix.conf
sandbox = true
sandbox-fallback = false
```

### Build Execution

Builds run in Nix sandboxes (if enabled):

```bash
# Verify sandbox is enabled
nix-build --max-jobs 4 --store /nix/store \
  --expr 'with import <nixpkgs> {}; hello'
# Should show: build for /nix/store/...-hello-2.10.drv

# Check nix.conf
grep "^sandbox" /etc/nix/nix.conf  # Should be 'true'
```

### Git Operations

ekapkgs-update clones and manipulates git repositories in temporary worktrees.

**Security considerations:**

1. **Verify repository contents** before merging PRs
2. **Be cautious with `git apply`** of patches from untrusted sources
3. **Audit git hooks** in your repositories
4. **Use signed commits** for merged PRs

### API Calls

Python and Rust dependencies can execute code when imported/evaluated.

**Mitigations:**

1. **Pin dependency versions** in Cargo.toml:

```toml
[dependencies]
tokio = "=1.35.0"  # Exact version, not "~1.35" or "1.35"
sqlx = "=0.7.3"
```

2. **Use dependency scanning**:

```bash
# Check for known vulnerabilities
cargo audit

# Update safely
cargo update
cargo test  # Verify updates don't break functionality
```

3. **Review transitive dependencies**:

```bash
cargo tree | grep -E "^[a-z-]"  # Show direct dependencies
cargo tree  # Show all including transitive
```

## File System Permissions

### Database Security

The SQLite database stores:
- Package version history
- Update attempt logs (includes error messages)
- Session configuration (may contain sensitive settings)

**Protect with file permissions:**

```bash
# Database location
ls -la ~/.cache/ekapkgs-update/updates.db

# Should be:
# -rw------- (600 permissions, owner only)

# Enforce
chmod 600 ~/.cache/ekapkgs-update/updates.db
chmod 700 ~/.cache/ekapkgs-update/
```

### Log Files

Logs may contain:
- API responses (versions, metadata, sometimes error details)
- Build output (including paths and errors)
- Configuration details

**Protect logs:**

```bash
# Location: ~/.config/ekapkgs-update/logs/
chmod 700 ~/.config/ekapkgs-update/logs/

# Archive securely
tar czf ekapkgs-logs-backup.tar.gz ~/.config/ekapkgs-update/logs/
shred -vfz -n 5 ~/.config/ekapkgs-update/logs/*  # Securely delete
```

### Artifact Preservation

Failure artifacts in `~/.cache/ekapkgs-update/failed/` may contain:

- Build system output (errors, build flags)
- Error messages (may reveal security issues)
- Package metadata (likely public, but still sensitive)
- Modified files from failed update attempt

**Manage access:**

```bash
# Restrict directory
chmod 700 ~/.cache/ekapkgs-update/failed/

# Before sharing with others, audit contents
ls ~/.cache/ekapkgs-update/failed/{session}/{attr}/

# Redact sensitive data
sed -i 's/secret-key-[a-z0-9]*/SECRET/g' error.log
```

## Deployment Security

### Systemd Service

If running as a systemd service, use restricted permissions:

```ini
# /etc/systemd/system/ekapkgs-update.service
[Service]
Type=oneshot
ExecStart=/usr/bin/ekapkgs-update run --config /etc/ekapkgs-update/config.toml

# Security hardening
NoNewPrivileges=yes
PrivateTmp=yes
ProtectHome=read-only  # Or 'yes' if not needed
ProtectSystem=strict
ReadWritePaths=/var/lib/ekapkgs-update

# User/group
User=ekapkgs-update
Group=ekapkgs-update
```

### NixOS Module

The ekapkgs-update NixOS module provides security hardening:

```nix
services.ekapkgs-update = {
  enable = true;
  user = "ekapkgs-update";
  group = "ekapkgs-update";

  environment = {
    GITHUB_TOKEN = "${config.sops.secrets.github-token.path}";
    CACHIX_AUTH_TOKEN = "${config.sops.secrets.cachix-token.path}";
  };

  # Recommended: use sops-nix for secrets management
};
```

### Container Deployment

If running in Docker/Podman:

```dockerfile
FROM nixos/nix:latest

# Add ekapkgs-update
RUN nix-shell -p ekapkgs-update --run "true"

# Create restricted user
RUN useradd -m -u 1000 updater

# Restrict container
USER updater
WORKDIR /home/updater

# Mount secrets as read-only
# VOLUME ["/run/secrets/github-token"]
```

Run with restrictions:

```bash
podman run \
  --rm \
  --cap-drop=ALL \
  --read-only \
  --read-write=/tmp \
  --read-write=/var/cache \
  -e GITHUB_TOKEN_FILE=/run/secrets/gh-token \
  -v ~/secrets/gh-token:/run/secrets/gh-token:ro \
  ekapkgs-update:latest \
  ekapkgs-update run --config config.toml
```

## Supply Chain Security

### Dependency Verification

Verify ekapkgs-update itself:

```bash
# Check source integrity
git verify-commit HEAD

# Build from source
cargo build --release
./target/release/ekapkgs-update --version

# Verify dependencies
cargo tree --all-features
cargo audit
```

### Reproducible Builds

Build ekapkgs-update reproducibly:

```bash
# Using Nix (reproducible)
nix build github:ekapusta/ekapkgs-update

# Using Cargo (not reproducible without flags)
RUSTFLAGS="-C target-cpu=generic" cargo build --release
```

## Secrets Management

### Configuration Secrets

Avoid putting secrets in config files:

```bash
# Bad: config.toml contains tokens
[secrets]
github-token = "ghp_xxx"

# Good: read from environment
# config.toml doesn't contain tokens
# export GITHUB_TOKEN="ghp_xxx" before running

# Best: use sops-nix or similar
# secrets/github-token.enc
```

### Sops-nix Integration

Manage secrets securely:

```nix
# secrets.nix (add PGP key)
let
  system_key = "EDD40C31B280DDDDDDDD";
  user_key = "F24EE10B67FA47DAABBBBBB";
in {
  "ekapkgs/github-token.enc" = {
    owner = "ekapkgs-update";
    inherit (system_key) groups;
  };
}

# flake.nix
sops.secrets."ekapkgs/github-token" = {};

services.ekapkgs-update = {
  environment.GITHUB_TOKEN = "\${config.sops.secrets."ekapkgs/github-token".path}";
};
```

Encrypt secrets:

```bash
sops -k EDD40C31B280DDDDDDDD secrets.yaml
# Opens in $EDITOR, automatically encrypts on save
```

## Audit and Monitoring

### Activity Logs

Monitor what ekapkgs-update does:

```bash
# View activity
journalctl -u ekapkgs-update -f

# Query past activity
journalctl -u ekapkgs-update --since "2024-05-01"

# Log to file
journalctl -u ekapkgs-update > ekapkgs-activity.log
```

### Database Audit

Query who made changes:

```sql
-- Most recent updates
SELECT attr_path, current_version, proposed_version, last_attempted
FROM updates
ORDER BY last_attempted DESC
LIMIT 10;

-- Failed attempts
SELECT attr_path, timestamp, error_log
FROM update_logs
WHERE timestamp > datetime('now', '-7 days')
ORDER BY timestamp DESC;
```

### PR Review

Monitor created PRs:

```bash
# GitHub CLI
gh pr list --creator ekapkgs-update --state all --limit 100

# Check contents manually
gh pr view {number} --json body,commits

# Audit commits
gh pr view {number} --json commits --jq '.commits[].oid'
```

## Incident Response

### Compromised Token

If a token is exposed:

```bash
# 1. Immediately revoke in GitHub/GitLab UI
# 2. Search for unauthorized access
gh api user/events --limit 30 | jq '.[] | select(.created_at > "2024-05-15")'

# 3. Close any suspicious PRs
gh pr close {suspicious_number} --delete-branch

# 4. Generate new token with minimal scope
# (Settings → Personal Access Tokens)

# 5. Update credential store
export GITHUB_TOKEN="ghp_new_token"

# 6. Document the incident
echo "Token compromised on 2024-05-15, rotated at 10:30" >> INCIDENT_LOG
```

### Suspicious Update Activity

If updates are being made unexpectedly:

```bash
# 1. Check git logs
git log --oneline --author=ekapkgs-update | head -20

# 2. Inspect suspicious commits
git show {commit-hash}

# 3. Revert if necessary
git revert {commit-hash}

# 4. Audit configuration
cat ~/.config/ekapkgs-update/config.toml

# 5. Check database
sqlite3 ~/.cache/ekapkgs-update/updates.db \
  "SELECT * FROM updates WHERE proposed_version IS NOT NULL LIMIT 5;"
```

## Best Practices Summary

1. **Rotate tokens quarterly** even if not compromised
2. **Use credential managers** instead of environment variables
3. **Restrict file permissions** on databases and logs (700/600)
4. **Enable Nix sandboxing** by default
5. **Review PRs carefully** before merging
6. **Audit dependencies** regularly with `cargo audit`
7. **Monitor activity logs** for unexpected changes
8. **Use signed commits** for merged updates
9. **Encrypt secrets** in configuration
10. **Test in staging** before production deployment

## Related Topics

- [NixOS Module](../nixos-module.md) - Deployment security
- [Systemd Service](../systemd.md) - Service hardening
- [Failure Preservation](./failure-preservation.md) - Artifact access control
- [Database Schema](./database.md) - Sensitive data in logs and metadata
