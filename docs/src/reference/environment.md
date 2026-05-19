# Environment Variables

Environment variables used by ekapkgs-update.

## Authentication Tokens

### GITHUB_TOKEN

GitHub personal access token for API access and PR creation.

**Required for:**
- Creating pull requests
- Higher API rate limits (5000/hour vs 60/hour)
- Accessing private repositories

**Format:** `ghp_` followed by 36 characters

**Scopes needed:**
- `repo` - Full repository access (for PR creation)
- `public_repo` - Public repository access (for read-only)

```bash
export GITHUB_TOKEN="ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
```

**Generate token:**
1. Visit https://github.com/settings/tokens
2. Click "Generate new token (classic)"
3. Select scopes: `repo`
4. Click "Generate token"
5. Copy token to environment

### GITLAB_TOKEN

GitLab personal access token for API access.

**Required for:**
- Fetching versions from GitLab projects
- Creating merge requests
- Higher API rate limits

**Scopes needed:**
- `api` - Full API access
- `read_api` - Read-only access (if not creating MRs)

```bash
export GITLAB_TOKEN="glpat-xxxxxxxxxxxxxxxxxxxxx"
```

**Generate token:**
1. Visit https://gitlab.com/-/profile/personal_access_tokens
2. Create new token with `api` scope
3. Copy token to environment

### SOURCEHUT_TOKEN

SourceHut OAuth token for API access.

**Required for:**
- Fetching versions from SourceHut repositories
- Higher API rate limits

```bash
export SOURCEHUT_TOKEN="xxxxxxxxxxxxxxxxxxxxx"
```

**Generate token:**
1. Visit https://meta.sr.ht/oauth
2. Create new OAuth token
3. Grant `repositories:read` scope
4. Copy token to environment

## Cachix

### CACHIX_AUTH_TOKEN

Cachix authentication token for pushing build outputs.

**Required for:**
- Pushing successful builds to Cachix

```bash
export CACHIX_AUTH_TOKEN="eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
```

**Get token:**
1. Visit https://app.cachix.org
2. Navigate to your cache settings
3. Copy auth token

### CACHIX_CACHE_NAME

Default Cachix cache name to push to.

**Optional:** Can be set via `--cachix-cache` flag instead

```bash
export CACHIX_CACHE_NAME="my-cache"
```

**Usage:**
```bash
# Uses CACHIX_CACHE_NAME environment variable
ekapkgs-update run

# Or specify via flag
ekapkgs-update run --cachix-cache my-cache
```

## Logging

### RUST_LOG

Controls logging verbosity using the `env_logger` format.

**Levels:**
- `error` - Only errors
- `warn` - Warnings and errors
- `info` - Informational messages (default)
- `debug` - Debug information
- `trace` - Very detailed tracing

```bash
# Basic usage
export RUST_LOG=info
export RUST_LOG=debug

# Module-specific
export RUST_LOG=ekapkgs_update=debug

# Multiple modules
export RUST_LOG=ekapkgs_update=debug,hyper=info

# Detailed tracing for specific module
export RUST_LOG=ekapkgs_update::vcs_sources=trace
```

**Examples:**
```bash
# Default (info level)
ekapkgs-update run

# Debug mode
RUST_LOG=debug ekapkgs-update run

# Only errors
RUST_LOG=error ekapkgs-update run

# Debug update module, info for everything else
RUST_LOG=ekapkgs_update::commands::update=debug,info ekapkgs-update run
```

## Database

### DATABASE_PATH

Override default database location.

**Default:** `~/.cache/ekapkgs-update/db.sqlite3`

```bash
export DATABASE_PATH=/var/lib/ekapkgs-update/db.sqlite3
```

**Usage:**
```bash
# Uses DATABASE_PATH environment variable
ekapkgs-update status

# Or specify via flag (overrides environment)
ekapkgs-update status --database /custom/path/db.sqlite3
```

## Nix Configuration

### NIX_PATH

Nix package path for evaluation.

```bash
export NIX_PATH=nixpkgs=/path/to/nixpkgs
```

### NIX_CONFIG

Additional Nix configuration.

```bash
export NIX_CONFIG="
  experimental-features = nix-command flakes
  max-jobs = auto
  cores = 0
"
```

## HTTP/Network

### HTTP_PROXY / HTTPS_PROXY

HTTP proxy for network requests.

```bash
export HTTP_PROXY=http://proxy.example.com:8080
export HTTPS_PROXY=http://proxy.example.com:8080
```

### NO_PROXY

Bypass proxy for specific hosts.

```bash
export NO_PROXY=localhost,127.0.0.1,.example.com
```

## Performance

### RAYON_NUM_THREADS

Override number of threads for parallel operations.

**Default:** Number of CPU cores

```bash
# Limit to 4 threads
export RAYON_NUM_THREADS=4

# Use all cores
export RAYON_NUM_THREADS=0
```

## Web Dashboard

### WEB_HOST

Web dashboard bind host.

**Default:** `127.0.0.1`

```bash
export WEB_HOST=0.0.0.0  # Listen on all interfaces
```

### WEB_PORT

Web dashboard port.

**Default:** `3000`

```bash
export WEB_PORT=8080
```

## Complete Example

### Development

```bash
# .env
export GITHUB_TOKEN="ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
export RUST_LOG=debug
export DATABASE_PATH="$HOME/.cache/ekapkgs-update/db.sqlite3"
```

### Production

```bash
# /etc/ekapkgs-update/environment
GITHUB_TOKEN=ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxx
GITLAB_TOKEN=glpat-xxxxxxxxxxxxxxxxxxxxx
CACHIX_AUTH_TOKEN=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...
CACHIX_CACHE_NAME=production
RUST_LOG=info
DATABASE_PATH=/var/lib/ekapkgs-update/db.sqlite3
WEB_HOST=127.0.0.1
WEB_PORT=3000
```

### CI/CD

```yaml
# .github/workflows/update.yml
env:
  GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
  RUST_LOG: info
  CACHIX_AUTH_TOKEN: ${{ secrets.CACHIX_AUTH_TOKEN }}
  CACHIX_CACHE_NAME: ci-cache
```

## Security Best Practices

### 1. Never Commit Tokens

```bash
# Add to .gitignore
echo ".env" >> .gitignore
echo "environment" >> .gitignore
```

### 2. Use Secret Management

```bash
# systemd with secret management
[Service]
EnvironmentFile=/run/secrets/ekapkgs-update/environment
```

```nix
# NixOS with agenix
{
  age.secrets.github-token = {
    file = ./secrets/github-token.age;
    owner = "ekapkgs";
  };

  services.ekapkgs-update = {
    enable = true;
    githubTokenFile = config.age.secrets.github-token.path;
  };
}
```

### 3. Restrict File Permissions

```bash
chmod 600 /etc/ekapkgs-update/environment
chown ekapkgs:ekapkgs /etc/ekapkgs-update/environment
```

### 4. Use Environment-Specific Tokens

```bash
# Different tokens for different environments
# Development
export GITHUB_TOKEN="ghp_dev_xxxxxxxx"

# Production
export GITHUB_TOKEN="ghp_prod_xxxxxxxx"

# CI
export GITHUB_TOKEN="${{ secrets.CI_GITHUB_TOKEN }}"
```

## Troubleshooting

### Token Issues

```bash
# Test GitHub token
curl -H "Authorization: token $GITHUB_TOKEN" https://api.github.com/user

# Test GitLab token
curl -H "PRIVATE-TOKEN: $GITLAB_TOKEN" https://gitlab.com/api/v4/user

# Check token scopes
curl -I -H "Authorization: token $GITHUB_TOKEN" https://api.github.com/users/octocat
# Look for X-OAuth-Scopes header
```

### Rate Limiting

```bash
# Check GitHub rate limit
curl -H "Authorization: token $GITHUB_TOKEN" https://api.github.com/rate_limit

# With token: 5000/hour
# Without token: 60/hour
```

### Proxy Issues

```bash
# Test proxy
export HTTP_PROXY=http://proxy.example.com:8080
curl https://api.github.com/users/octocat

# Bypass proxy for specific domain
export NO_PROXY=github.com
```

## See Also

- [Installation](../installation.md) - Setup instructions
- [CI/CD Integration](../use-cases/ci-cd.md) - Environment in pipelines
- [Troubleshooting](./troubleshooting.md) - Common issues
