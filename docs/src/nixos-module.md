# NixOS Module

The ekapkgs-update NixOS module provides a systemd service for automated package updates with full secret management, state directory handling, and optional web dashboard.

## Basic Setup

### Minimal Configuration

```nix
{
  services.ekapkgs-update = {
    enable = true;
    packagesFile = ./packages/default.nix;
  };
}
```

This creates a systemd service running as user `eka-ci` that:
- Evaluates `/path/to/packages/default.nix`
- Stores database in `/var/lib/ekapkgs-update/updates.db`
- Runs continuously, checking for updates

### Complete Configuration Example

```nix
{ config, ... }:

{
  services.ekapkgs-update = {
    enable = true;

    # Package to use (defaults to pkgs.ekapkgs-update)
    package = pkgs.ekapkgs-update;

    # File containing packages to update
    packagesFile = ./my-packages/default.nix;

    # User/group for the service
    user = "eka-ci";  # default
    group = "eka-ci"; # default

    # State directory under /var/lib/
    stateDirectory = "ekapkgs-update"; # default

    # Environment variables file (for secrets)
    environmentFile = config.sops.secrets.ekapkgs-tokens.path;

    # Additional CLI arguments
    extraArgs = [
      "--max-rebuilds" "200"
      "--skip-unstable"
      "--concurrent-updates" "6"
      "--preserve-failures"
      "--analyze-rebuilds"
    ];

    # Cachix configuration
    cachix = {
      cacheName = "my-cache";
      # Option 1: Use environmentFile for token
      # (CACHIX_AUTH_TOKEN=... in environmentFile)

      # Option 2: Use dedicated auth token file
      authTokenFile = config.sops.secrets.cachix-token.path;
    };

    # Web dashboard
    web = {
      enable = true;
      host = "0.0.0.0";  # Listen on all interfaces
      port = 3000;
      cors = true;
      openFirewall = true;  # Automatically open port 3000
    };
  };
}
```

## Configuration Options

### Core Options

#### `enable`
- **Type**: `bool`
- **Default**: `false`
- **Description**: Enable the ekapkgs-update service

#### `package`
- **Type**: `package`
- **Default**: `pkgs.ekapkgs-update`
- **Description**: The ekapkgs-update package to use

#### `packagesFile`
- **Type**: `path`
- **Required**: Yes
- **Example**: `./default.nix`
- **Description**: Path to the Nix file containing packages to update

### User & State Management

#### `user`
- **Type**: `str`
- **Default**: `"eka-ci"`
- **Description**: System user the daemon runs as (created automatically)

#### `group`
- **Type**: `str`
- **Default**: `"eka-ci"`
- **Description**: System group the daemon runs as (created automatically)

#### `stateDirectory`
- **Type**: `str`
- **Default**: `"ekapkgs-update"`
- **Description**: Directory under `/var/lib/` for database and state

### Secret Management

#### `environmentFile`
- **Type**: `nullOr path`
- **Default**: `null`
- **Description**: File with environment variables in `KEY=VALUE` format

This is the **recommended** way to provide secrets. Compatible with sops-nix, agenix, and other secret managers.

**Example with sops-nix**:
```nix
{
  sops.secrets.ekapkgs-tokens = {
    sopsFile = ./secrets.yaml;
    owner = config.services.ekapkgs-update.user;
  };

  services.ekapkgs-update = {
    enable = true;
    environmentFile = config.sops.secrets.ekapkgs-tokens.path;
  };
}
```

**environmentFile contents**:
```bash
GITHUB_TOKEN=ghp_xxxxxxxxxxxx
CACHIX_AUTH_TOKEN=eyJhbGc...
CACHIX_CACHE_NAME=my-cache
GITLAB_TOKEN=glpat-xxxxxxxxxxxx
SOURCEHUT_TOKEN=~/.config/hut/token
```

### CLI Arguments

#### `extraArgs`
- **Type**: `listOf str`
- **Default**: `[]`
- **Example**: `["--skip-cve" "--max-rebuilds" "100"]`
- **Description**: Additional arguments passed to `ekapkgs-update run`

All [run command options](./cli/run.md) are supported:
- `--max-rebuilds N` - Skip updates causing >N rebuilds
- `--skip-unstable` - Skip packages with "unstable" in version
- `--concurrent-updates N` - Number of parallel update workers
- `--run-passthru-tests` - Run passthru.tests before accepting updates
- `--preserve-failures` - Keep failed worktrees for debugging
- `--analyze-rebuilds` - Report rebuild counts in PR descriptions
- `--skip-cve` - Skip CVE vulnerability checking
- `--skip-repology` - Skip Repology cross-distribution checks
- `--skip-directory-diff` - Skip directory diff in PR descriptions
- `--skip-cachix` - Skip Cachix push (even if token available)
- `--interactive` - Prompt before submitting PRs

### Cachix Integration

#### `cachix.cacheName`
- **Type**: `nullOr str`
- **Default**: `null`
- **Example**: `"ekapkgs"`
- **Description**: Cachix cache name to push successful builds to

#### `cachix.authTokenFile`
- **Type**: `nullOr path`
- **Default**: `null`
- **Description**: File containing bare Cachix auth token (alternative to environmentFile)

**Two options for Cachix auth**:

**Option 1: environmentFile** (recommended if you have multiple secrets):
```nix
services.ekapkgs-update = {
  environmentFile = config.sops.secrets.tokens.path;
  cachix.cacheName = "my-cache";
};
# tokens file contains: CACHIX_AUTH_TOKEN=...
```

**Option 2: authTokenFile** (for stricter credential isolation):
```nix
services.ekapkgs-update = {
  cachix = {
    cacheName = "my-cache";
    authTokenFile = config.sops.secrets.cachix-token.path;
  };
};
```

### Web Dashboard

#### `web.enable`
- **Type**: `bool`
- **Default**: `false`
- **Description**: Enable the web monitoring portal

#### `web.package`
- **Type**: `package`
- **Default**: `pkgs.ekapkgs-update-web`
- **Description**: The web dashboard package to use

#### `web.host`
- **Type**: `str`
- **Default**: `"127.0.0.1"`
- **Example**: `"0.0.0.0"`
- **Description**: Host address to bind to (use `0.0.0.0` for all interfaces)

#### `web.port`
- **Type**: `port`
- **Default**: `3000`
- **Description**: Port for web server

#### `web.cors`
- **Type**: `bool`
- **Default**: `false`
- **Description**: Enable CORS headers (needed for reverse proxies)

#### `web.user` / `web.group`
- **Type**: `str`
- **Default**: Same as main service user/group
- **Description**: User/group for web portal (must have database read access)

#### `web.database`
- **Type**: `str`
- **Default**: `/var/lib/${stateDirectory}/updates.db`
- **Description**: Path to SQLite database

#### `web.openFirewall`
- **Type**: `bool`
- **Default**: `false`
- **Description**: Automatically open firewall for web.port

## Complete Examples

### Example 1: Basic with sops-nix

```nix
{ config, ... }:

{
  # Configure secrets
  sops.secrets.ekapkgs-env = {
    sopsFile = ./secrets/ekapkgs.yaml;
    owner = "eka-ci";
    group = "eka-ci";
  };

  # Configure service
  services.ekapkgs-update = {
    enable = true;
    packagesFile = ./packages/default.nix;
    environmentFile = config.sops.secrets.ekapkgs-env.path;

    extraArgs = [
      "--max-rebuilds" "150"
      "--concurrent-updates" "4"
      "--preserve-failures"
    ];

    cachix.cacheName = "my-nixpkgs-fork";
  };
}
```

### Example 2: With Web Dashboard and Reverse Proxy

```nix
{ config, ... }:

{
  services.ekapkgs-update = {
    enable = true;
    packagesFile = /srv/packages/default.nix;
    environmentFile = config.age.secrets.ekapkgs-tokens.path;

    extraArgs = [
      "--max-rebuilds" "200"
      "--analyze-rebuilds"
    ];

    web = {
      enable = true;
      host = "127.0.0.1";  # Only accessible via reverse proxy
      port = 3000;
      cors = true;
    };
  };

  # Nginx reverse proxy
  services.nginx = {
    enable = true;
    virtualHosts."updates.example.com" = {
      enableACME = true;
      forceSSL = true;
      locations."/" = {
        proxyPass = "http://127.0.0.1:3000";
        proxyWebsockets = true;
      };
    };
  };
}
```

### Example 3: High-throughput CI Server

```nix
{ config, pkgs, ... }:

{
  services.ekapkgs-update = {
    enable = true;
    packagesFile = /var/lib/ekapkgs-update/packages.nix;

    environmentFile = config.sops.secrets.ci-tokens.path;

    extraArgs = [
      "--max-rebuilds" "500"           # High threshold
      "--concurrent-updates" "16"       # Lots of parallelism
      "--preserve-failures"             # Keep artifacts
      "--run-passthru-tests"           # Validate everything
      "--analyze-rebuilds"              # Detailed PR info
      "--skip-unstable"                 # Skip development versions
    ];

    cachix = {
      cacheName = "company-nixpkgs";
      authTokenFile = config.sops.secrets.cachix-ci-token.path;
    };

    web = {
      enable = true;
      host = "0.0.0.0";
      port = 8080;
      cors = true;
      openFirewall = true;
    };
  };

  # Ensure fast builds with remote builders
  nix.buildMachines = [
    {
      hostName = "builder1.example.com";
      system = "x86_64-linux";
      maxJobs = 8;
      speedFactor = 2;
      supportedFeatures = [ "nixos-test" "benchmark" "big-parallel" "kvm" ];
    }
  ];
}
```

## Service Management

### Controlling the Service

```bash
# Start service
sudo systemctl start ekapkgs-update.service

# Stop service
sudo systemctl stop ekapkgs-update.service

# Restart service
sudo systemctl restart ekapkgs-update.service

# View status
sudo systemctl status ekapkgs-update.service

# View logs
sudo journalctl -u ekapkgs-update.service -f

# View web dashboard logs
sudo journalctl -u ekapkgs-update-web.service -f
```

### Database Location

The database is stored at:
```
/var/lib/${stateDirectory}/updates.db
```

Default: `/var/lib/ekapkgs-update/updates.db`

Access requires appropriate permissions (user `eka-ci` by default).

### Checking Status

```bash
# As the service user
sudo -u eka-ci ekapkgs-update status \
  --database /var/lib/ekapkgs-update/updates.db

# Query recent failures
sudo -u eka-ci ekapkgs-update query \
  --database /var/lib/ekapkgs-update/updates.db \
  --since-days 7
```

## Security Considerations

### Systemd Hardening

The module applies comprehensive systemd hardening:

- `NoNewPrivileges=true` - Prevent privilege escalation
- `ProtectSystem=strict` - Read-only system directories
- `ProtectHome=true` - Hide home directories
- `PrivateTmp=true` - Private /tmp
- `PrivateDevices=true` - Restricted device access
- `ProtectKernelTunables=true` - Protect /proc/sys
- `ProtectKernelModules=true` - Prevent module loading
- `ProtectControlGroups=true` - Protect cgroup filesystem
- `RestrictSUIDSGID=true` - Ignore SUID/SGID bits
- `LockPersonality=true` - Lock execution domain

### Credential Handling

**Recommended patterns**:

1. **sops-nix**: Encrypted secrets in repository
   ```nix
   sops.secrets.ekapkgs-tokens = {
     sopsFile = ./secrets.yaml;
     owner = "eka-ci";
   };
   ```

2. **agenix**: Age-encrypted secrets
   ```nix
   age.secrets.ekapkgs-env = {
     file = ./secrets/ekapkgs.age;
     owner = "eka-ci";
   };
   ```

3. **systemd LoadCredential**: For authTokenFile
   - Loaded only in service namespace
   - Not visible to other processes

**Never**:
- Hard-code tokens in configuration.nix
- Store tokens in world-readable files
- Commit tokens to git

## Troubleshooting

### Service won't start

```bash
# Check service status
sudo systemctl status ekapkgs-update.service

# View recent logs
sudo journalctl -u ekapkgs-update.service -n 50

# Check for configuration errors
sudo nixos-rebuild dry-build
```

### Permission denied errors

Ensure the service user has access to:
- `/var/lib/ekapkgs-update/` (created automatically)
- `packagesFile` path (must be readable)
- `environmentFile` (check owner/permissions)

### Database locked

Only one instance should run at a time. Check:
```bash
# Kill any hung processes
sudo systemctl stop ekapkgs-update.service

# Check for stale locks
sudo lsof /var/lib/ekapkgs-update/updates.db
```

### Web dashboard not accessible

```bash
# Check web service status
sudo systemctl status ekapkgs-update-web.service

# Verify port is listening
sudo ss -tlnp | grep 3000

# Check firewall (if openFirewall = true)
sudo iptables -L | grep 3000
```

## Next Steps

- [Systemd Service](./systemd.md) - Manual systemd setup without NixOS module
- [Web Dashboard](./web-dashboard.md) - Using the monitoring interface
- [Configuration](./configuration.md) - Repository-specific configuration
