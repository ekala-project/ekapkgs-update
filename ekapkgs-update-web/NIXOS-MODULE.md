# NixOS Module for ekapkgs-update-web

The ekapkgs-update web portal can be easily deployed on NixOS using the integrated module.

## Quick Start

Add to your NixOS configuration:

```nix
{
  # Import the flake in your flake.nix inputs
  inputs.ekapkgs-update.url = "github:ekala-project/ekapkgs-update";

  # In your configuration.nix or module
  imports = [
    inputs.ekapkgs-update.nixosModules.default
  ];

  services.ekapkgs-update = {
    enable = true;
    packagesFile = ./my-packages.nix;

    # Enable the web portal
    web.enable = true;
  };
}
```

That's it! The web portal will be available at `http://localhost:3000`.

## Configuration Options

### Basic Web Portal Options

```nix
services.ekapkgs-update.web = {
  # Enable the web portal
  enable = true;

  # Host to bind to (default: 127.0.0.1)
  host = "0.0.0.0";  # Listen on all interfaces

  # Port to listen on (default: 3000)
  port = 8080;

  # Enable CORS headers for public access
  cors = true;

  # Automatically open firewall port
  openFirewall = true;
};
```

### Advanced Options

```nix
services.ekapkgs-update.web = {
  enable = true;

  # Override the package (useful for testing)
  package = pkgs.ekapkgs-update-web;

  # Custom database path (defaults to main service database)
  database = "/custom/path/to/updates.db";

  # Run as specific user/group (defaults to main service user)
  user = "my-web-user";
  group = "my-web-group";
};
```

## Example Configurations

### 1. Local Development

```nix
services.ekapkgs-update = {
  enable = true;
  packagesFile = ./packages.nix;

  web = {
    enable = true;
    # Defaults are fine: localhost:3000
  };
};
```

Access at: `http://localhost:3000`

### 2. Internal Team Dashboard

```nix
services.ekapkgs-update = {
  enable = true;
  packagesFile = ./packages.nix;

  web = {
    enable = true;
    host = "0.0.0.0";  # Listen on all interfaces
    port = 3000;
    openFirewall = true;  # Allow access from network
  };
};
```

Access at: `http://your-server-ip:3000`

### 3. Public Portal with Reverse Proxy

```nix
services.ekapkgs-update = {
  enable = true;
  packagesFile = ./packages.nix;

  web = {
    enable = true;
    host = "127.0.0.1";  # Only localhost
    port = 3000;
    cors = true;  # Enable CORS for reverse proxy
  };
};

# Add nginx reverse proxy
services.nginx = {
  enable = true;
  virtualHosts."updates.example.com" = {
    enableACME = true;
    forceSSL = true;
    locations."/" = {
      proxyPass = "http://127.0.0.1:3000";
      proxyWebsockets = true;  # Important for WebSocket support!
      extraConfig = ''
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
      '';
    };
  };
};

security.acme.defaults.email = "admin@example.com";
```

Access at: `https://updates.example.com`

### 4. Separate Database (Manual Setup)

```nix
services.ekapkgs-update = {
  enable = true;
  packagesFile = ./packages.nix;
  # Main service uses default database at /var/lib/ekapkgs-update/updates.db
};

# Separate web portal instance reading from a different database
services.ekapkgs-update.web = {
  enable = true;
  database = "/custom/path/other-updates.db";
  port = 4000;
};
```

### 5. Multiple Instances (Different Ports)

Unfortunately, the current module structure only supports one web portal instance per NixOS configuration. To run multiple instances, you would need to manually define additional systemd services or use NixOS containers.

## Integration with Main Service

The web portal automatically shares configuration with the main `ekapkgs-update` service:

- **User/Group**: Defaults to the same user as the main service (`eka-ci`)
- **Database**: Defaults to `/var/lib/${stateDirectory}/updates.db`
- **State Directory**: Automatically has read access to the main service's state

This means:
1. ✅ No additional user/group setup needed
2. ✅ Automatic database access
3. ✅ Shares the same database by default
4. ✅ No manual permissions configuration

## Systemd Service

The module creates a systemd service: `ekapkgs-update-web.service`

### Managing the Service

```bash
# Check status
systemctl status ekapkgs-update-web

# View logs
journalctl -u ekapkgs-update-web -f

# Restart
systemctl restart ekapkgs-update-web

# Stop/Start
systemctl stop ekapkgs-update-web
systemctl start ekapkgs-update-web
```

### Service Features

- **Auto-restart**: Restarts on failure with 10s delay
- **Network dependency**: Waits for `network-online.target`
- **Hardening**: Extensive systemd security hardening applied
  - Read-only filesystem except database directory
  - No new privileges
  - Protected home, kernel, devices
  - Restricted syscalls and namespaces
- **Read-only database access**: Only needs read access to the database

## Firewall Configuration

### Manual Firewall

If not using `openFirewall = true`:

```nix
networking.firewall.allowedTCPPorts = [ 3000 ];
```

### With UFW

```bash
sudo ufw allow 3000/tcp
```

## Security Considerations

### Read-Only Access

The web portal has **read-only** access to the database:
- ✅ Safe for public deployment
- ✅ Cannot modify update data
- ✅ Cannot interfere with main service
- ✅ Extensive systemd hardening applied

### Recommended Security Practices

1. **Use Reverse Proxy**: Run behind nginx/caddy with SSL
2. **Restrict Network**: Use firewall rules to limit access
3. **Enable CORS Carefully**: Only enable if needed for your deployment
4. **Monitor Logs**: Watch for suspicious access patterns
5. **Keep Updated**: Regularly update to get security fixes

### Authentication

The web portal does **not** include built-in authentication in the current version. For authenticated access:

1. **Use Nginx Basic Auth**:
```nix
services.nginx.virtualHosts."updates.example.com" = {
  locations."/" = {
    proxyPass = "http://127.0.0.1:3000";
    basicAuthFile = "/path/to/htpasswd";
  };
};
```

2. **Use Authelia/Authentik**: Deploy an authentication proxy
3. **Use VPN**: Restrict access via WireGuard/Tailscale

## Monitoring

### Prometheus Metrics (Future)

Currently, the web portal doesn't export Prometheus metrics, but you can monitor:

```bash
# Service status
systemctl is-active ekapkgs-update-web

# Check if port is listening
ss -tlnp | grep 3000

# Check logs for errors
journalctl -u ekapkgs-update-web --since "1 hour ago" -p err
```

## Troubleshooting

### Web Portal Won't Start

```bash
# Check service status
systemctl status ekapkgs-update-web

# View full logs
journalctl -u ekapkgs-update-web -b

# Check permissions
ls -la /var/lib/ekapkgs-update/
```

### Can't Access from Network

```bash
# Check if service is listening
ss -tlnp | grep 3000

# Check firewall
sudo iptables -L -n | grep 3000

# Verify host binding
# Should show 0.0.0.0:3000 (all interfaces) or specific IP
```

### WebSocket Issues with Reverse Proxy

Ensure nginx has WebSocket support:
```nix
locations."/" = {
  proxyPass = "http://127.0.0.1:3000";
  proxyWebsockets = true;  # Critical!
};
```

### Empty Dashboard

If the web portal starts but shows no data:
1. Ensure main service is running: `systemctl status ekapkgs-update`
2. Check database exists: `ls -la /var/lib/ekapkgs-update/updates.db`
3. Run an update to populate data: `systemctl start ekapkgs-update`

## Complete Example Configuration

Here's a complete, production-ready configuration:

```nix
{ config, pkgs, inputs, ... }:

{
  imports = [
    inputs.ekapkgs-update.nixosModules.default
  ];

  # Main update service
  services.ekapkgs-update = {
    enable = true;
    packagesFile = ./my-packages.nix;

    # Optional: Configure Cachix
    cachix.cacheName = "my-cache";
    environmentFile = "/run/secrets/ekapkgs-update-env";

    # Enable web portal
    web = {
      enable = true;
      host = "127.0.0.1";  # Behind reverse proxy
      port = 3000;
      cors = true;
    };
  };

  # Reverse proxy with SSL
  services.nginx = {
    enable = true;
    recommendedTlsSettings = true;
    recommendedOptimisation = true;
    recommendedGzipSettings = true;
    recommendedProxySettings = true;

    virtualHosts."updates.mycompany.com" = {
      enableACME = true;
      forceSSL = true;

      locations."/" = {
        proxyPass = "http://127.0.0.1:3000";
        proxyWebsockets = true;

        # Optional: Basic auth
        basicAuthFile = config.age.secrets.updates-htpasswd.path;

        extraConfig = ''
          proxy_set_header Host $host;
          proxy_set_header X-Real-IP $remote_addr;
          proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
          proxy_set_header X-Forwarded-Proto $scheme;
        '';
      };
    };
  };

  security.acme = {
    defaults.email = "admin@mycompany.com";
    acceptTerms = true;
  };

  # Firewall (nginx will be exposed)
  networking.firewall.allowedTCPPorts = [ 80 443 ];
}
```

## Package Outputs

The flake provides two packages:

```bash
# Main CLI tool
nix run github:ekala-project/ekapkgs-update

# Web portal
nix run github:ekala-project/ekapkgs-update#ekapkgs-update-web -- --help
```

## See Also

- [Main README](../README.md) - General project documentation
- [Web Portal README](./README.md) - Web portal features and usage
- [Web Server Guide](./WEB-SERVER.md) - Deployment scenarios and details
