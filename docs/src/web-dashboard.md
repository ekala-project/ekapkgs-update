# Web Dashboard

The web dashboard provides a browser-based interface for monitoring and managing package updates.

## Overview

Features:
- Real-time update session monitoring
- Failure analysis and visualization
- Package update history
- Error type distribution charts
- Interactive failure investigation
- One-click retry functionality

## Installation

### NixOS Module

```nix
# configuration.nix
{
  services.ekapkgs-update = {
    enable = true;
    web = {
      enable = true;
      port = 3000;
      host = "127.0.0.1";  # localhost only
    };
  };
}
```

### Standalone

```bash
# Build and run
nix build .#ekapkgs-update-web
./result/bin/ekapkgs-update-web \
  --database ~/.cache/ekapkgs-update/db.sqlite3 \
  --port 3000 \
  --host 0.0.0.0
```

### Docker

```bash
# Run web dashboard in Docker
docker run -d \
  -p 3000:3000 \
  -v ~/.cache/ekapkgs-update:/data \
  ekapkgs-update-web \
  --database /data/db.sqlite3 \
  --host 0.0.0.0
```

## Accessing the Dashboard

### Local Access

```bash
# Start the web server
ekapkgs-update-web --port 3000

# Open in browser
xdg-open http://localhost:3000
```

### Remote Access

For production deployments, use a reverse proxy:

#### Nginx

```nginx
# /etc/nginx/sites-available/ekapkgs-update
server {
    listen 80;
    server_name updates.example.com;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

#### Caddy

```caddy
# Caddyfile
updates.example.com {
    reverse_proxy localhost:3000
}
```

## Dashboard Sections

### 1. Overview Page

**URL:** `/`

**Displays:**
- Current update session status
- Recent activity summary
- Success/failure statistics
- Quick links to failures

**Use cases:**
- Monitor running updates
- Check system health at a glance
- Identify urgent issues

### 2. Sessions Page

**URL:** `/sessions`

**Displays:**
- List of all update sessions
- Session statistics (success/fail counts)
- Duration and timing
- Session status (running/completed/failed)

**Actions:**
- View session details
- Filter by date range
- Compare sessions

### 3. Failures Page

**URL:** `/failures`

**Displays:**
- All failed updates
- Grouped by error type
- Timeline view
- Trend charts

**Filters:**
- Error type
- Time range
- Package name
- Status

**Actions:**
- View failure details
- Export failure context
- Retry failed update

### 4. Packages Page

**URL:** `/packages`

**Displays:**
- All tracked packages
- Update history per package
- Current version
- Success rate

**Filters:**
- Package name search
- Status filter
- Update frequency

### 5. Analytics Page

**URL:** `/analytics`

**Displays:**
- Error type distribution (pie chart)
- Updates over time (line graph)
- Success rate trends
- Top failing packages
- Phase failure analysis

**Features:**
- Interactive charts
- Date range selection
- Export data as CSV/JSON

### 6. Package Detail Page

**URL:** `/package/:name`

**Displays:**
- Package information
- Version history
- Update attempts
- Test results
- Build logs

**Actions:**
- View logs
- Retry update
- Export context
- Compare versions

## Features

### Real-Time Updates

The dashboard uses WebSocket connections for real-time updates:

```javascript
// Automatically updates when:
// - New update session starts
// - Package update completes
// - Failure occurs
// - Session ends
```

**Indicators:**
- Live progress bars
- Real-time counters
- Status badges
- Activity feed

### Failure Investigation

Click on any failure to view:
- Full error message and stack trace
- Build log with syntax highlighting
- Modified files with diff view
- Suggested fixes
- Related failures

### Retry Functionality

From the web UI:
1. Navigate to failure
2. Click "Retry" button
3. Optionally apply patch
4. Monitor retry progress
5. View results

### Export Features

Export data for external analysis:
- **JSON:** Machine-readable format
- **CSV:** Spreadsheet import
- **Markdown:** Documentation/reports
- **PDF:** Printable reports (planned)

## Configuration

### Environment Variables

```bash
# Database location
DATABASE_PATH=/var/lib/ekapkgs-update/db.sqlite3

# Web server
WEB_HOST=127.0.0.1
WEB_PORT=3000

# Authentication (optional)
AUTH_ENABLED=false
AUTH_USERNAME=admin
AUTH_PASSWORD_HASH=...

# Session
SESSION_SECRET=random-secret-key
```

### Configuration File

```yaml
# config.yml
database:
  path: /var/lib/ekapkgs-update/db.sqlite3

web:
  host: 0.0.0.0
  port: 3000
  base_path: /updates  # For reverse proxy sub-paths

authentication:
  enabled: true
  method: basic  # basic, oauth, oidc
  users:
    - username: admin
      password_hash: $2b$...

limits:
  max_sessions_display: 100
  max_failures_display: 500
  chart_data_points: 365
```

## Security

### Authentication

Enable basic authentication:

```nix
{
  services.ekapkgs-update.web = {
    enable = true;
    authentication = {
      enable = true;
      username = "admin";
      passwordFile = "/run/secrets/ekapkgs-web-password";
    };
  };
}
```

### Network Access

Restrict access:

```nix
{
  services.ekapkgs-update.web = {
    enable = true;
    host = "127.0.0.1";  # localhost only

    # Use reverse proxy for external access
  };

  services.nginx = {
    enable = true;
    virtualHosts."updates.example.com" = {
      locations."/" = {
        proxyPass = "http://127.0.0.1:3000";
        extraConfig = ''
          # IP whitelist
          allow 10.0.0.0/8;
          deny all;
        '';
      };
    };
  };
}
```

### HTTPS

Always use HTTPS for production:

```nix
{
  services.nginx.virtualHosts."updates.example.com" = {
    enableACME = true;
    forceSSL = true;
    locations."/" = {
      proxyPass = "http://127.0.0.1:3000";
    };
  };
}
```

## API Endpoints

The dashboard exposes a REST API:

### GET /api/sessions

List all update sessions.

```bash
curl http://localhost:3000/api/sessions
```

### GET /api/failures

List failures with filters.

```bash
curl "http://localhost:3000/api/failures?since=7d&error_type=BuildFailure"
```

### GET /api/packages

List all packages.

```bash
curl http://localhost:3000/api/packages
```

### GET /api/package/:name

Get package details.

```bash
curl http://localhost:3000/api/package/python312Packages.requests
```

### POST /api/retry/:package

Retry failed update.

```bash
curl -X POST http://localhost:3000/api/retry/mypackage \
  -H "Content-Type: application/json" \
  -d '{"from_phase": "Build"}'
```

### GET /api/export/:package

Export failure context.

```bash
curl http://localhost:3000/api/export/mypackage?format=json > context.json
```

## Troubleshooting

### Dashboard Won't Start

```bash
# Check if port is in use
netstat -tlnp | grep 3000

# Check database access
sqlite3 ~/.cache/ekapkgs-update/db.sqlite3 "SELECT COUNT(*) FROM update_sessions;"

# Check logs
journalctl -u ekapkgs-update-web.service -f
```

### Data Not Updating

```bash
# Verify database is being written to
ekapkgs-update status

# Check web server is reading correct database
ekapkgs-update-web --database /path/to/db.sqlite3 --verbose
```

### Authentication Issues

```bash
# Reset password
htpasswd -c /etc/ekapkgs-update/htpasswd admin

# Or disable authentication temporarily
ekapkgs-update-web --no-auth
```

## See Also

- [NixOS Module](./nixos-module.md) - Declarative setup
- [Systemd Service](./systemd.md) - Manual service configuration
- [status command](./cli/status.md) - CLI alternative
- [query command](./cli/query.md) - CLI querying
