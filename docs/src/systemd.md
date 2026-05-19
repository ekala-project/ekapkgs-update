# Manual Systemd Service Setup

Set up ekapkgs-update as a systemd service without using the NixOS module.

## Service File

### Basic Service

```ini
# /etc/systemd/system/ekapkgs-update.service
[Unit]
Description=ekapkgs package updater
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
User=ekapkgs
Group=ekapkgs
WorkingDirectory=/var/lib/ekapkgs

# Environment variables
Environment="GITHUB_TOKEN=ghp_..."
Environment="CACHIX_AUTH_TOKEN=..."
Environment="RUST_LOG=info"

# Command
ExecStart=/usr/local/bin/ekapkgs-update run \
  --file /var/lib/ekapkgs/pkgs/default.nix \
  --database /var/lib/ekapkgs/db.sqlite3 \
  --skip-unstable \
  --run-passthru-tests \
  --max-rebuilds 100 \
  --cachix-cache production \
  --concurrent-updates 8

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=ekapkgs-update

# Resource limits
MemoryMax=16G
CPUQuota=400%

# Restart policy
Restart=on-failure
RestartSec=10m

[Install]
WantedBy=multi-user.target
```

### Environment File

For better security, use an environment file:

```ini
# /etc/systemd/system/ekapkgs-update.service
[Unit]
Description=ekapkgs package updater
After=network-online.target

[Service]
Type=oneshot
User=ekapkgs
Group=ekapkgs
WorkingDirectory=/var/lib/ekapkgs

# Load environment from file
EnvironmentFile=/etc/ekapkgs-update/environment

ExecStart=/usr/local/bin/ekapkgs-update run \
  --file /var/lib/ekapkgs/pkgs/default.nix \
  --database /var/lib/ekapkgs/db.sqlite3 \
  --skip-unstable \
  --max-rebuilds 100

StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

```bash
# /etc/ekapkgs-update/environment
GITHUB_TOKEN=ghp_xxxxxxxxxxxxx
CACHIX_AUTH_TOKEN=eyJhbGciOi...
RUST_LOG=info
```

```bash
# Secure the environment file
chmod 600 /etc/ekapkgs-update/environment
chown ekapkgs:ekapkgs /etc/ekapkgs-update/environment
```

## Timer File

### Daily Updates

```ini
# /etc/systemd/system/ekapkgs-update.timer
[Unit]
Description=Daily package updates
Requires=ekapkgs-update.service

[Timer]
# Run daily at 2 AM
OnCalendar=daily
OnCalendar=02:00

# Run on boot if missed
Persistent=true

# Randomize start time by up to 30 minutes
RandomizedDelaySec=30min

[Install]
WantedBy=timers.target
```

### Multiple Schedules

```ini
# /etc/systemd/system/ekapkgs-update.timer
[Unit]
Description=Package update schedule
Requires=ekapkgs-update.service

[Timer]
# Weekdays at 2 AM
OnCalendar=Mon-Fri 02:00

# Weekends at 6 AM
OnCalendar=Sat,Sun 06:00

# Also run every 12 hours
OnCalendar=*-*-* 02,14:00:00

Persistent=true

[Install]
WantedBy=timers.target
```

## Installation

### 1. Create User

```bash
# Create service user
sudo useradd -r -s /sbin/nologin -d /var/lib/ekapkgs ekapkgs

# Create directories
sudo mkdir -p /var/lib/ekapkgs
sudo chown ekapkgs:ekapkgs /var/lib/ekapkgs
```

### 2. Install ekapkgs-update

```bash
# Using Nix profile
sudo -u ekapkgs nix profile install github:ekapkgs/ekapkgs-update

# Or copy binary
sudo cp ./result/bin/ekapkgs-update /usr/local/bin/
sudo chmod +x /usr/local/bin/ekapkgs-update
```

### 3. Configure Environment

```bash
# Create config directory
sudo mkdir -p /etc/ekapkgs-update

# Create environment file
sudo cat > /etc/ekapkgs-update/environment << 'EOF'
GITHUB_TOKEN=ghp_xxxxxxxxxxxxx
CACHIX_AUTH_TOKEN=eyJhbGciOi...
RUST_LOG=info
DATABASE_PATH=/var/lib/ekapkgs/db.sqlite3
EOF

sudo chmod 600 /etc/ekapkgs-update/environment
sudo chown ekapkgs:ekapkgs /etc/ekapkgs-update/environment
```

### 4. Install Service Files

```bash
# Copy service and timer files
sudo cp ekapkgs-update.service /etc/systemd/system/
sudo cp ekapkgs-update.timer /etc/systemd/system/

# Reload systemd
sudo systemctl daemon-reload
```

### 5. Enable and Start

```bash
# Enable timer
sudo systemctl enable ekapkgs-update.timer

# Start timer
sudo systemctl start ekapkgs-update.timer

# Verify timer is active
sudo systemctl status ekapkgs-update.timer
```

## Management

### Check Status

```bash
# Timer status
sudo systemctl status ekapkgs-update.timer

# Service status
sudo systemctl status ekapkgs-update.service

# View next run time
sudo systemctl list-timers ekapkgs-update.timer
```

### Manual Run

```bash
# Trigger service manually
sudo systemctl start ekapkgs-update.service

# Check status
sudo systemctl status ekapkgs-update.service
```

### View Logs

```bash
# Follow live logs
sudo journalctl -u ekapkgs-update.service -f

# View recent logs
sudo journalctl -u ekapkgs-update.service -n 100

# View logs since yesterday
sudo journalctl -u ekapkgs-update.service --since yesterday

# View logs with priority
sudo journalctl -u ekapkgs-update.service -p err
```

### Restart Timer

```bash
# Restart timer
sudo systemctl restart ekapkgs-update.timer

# Stop timer
sudo systemctl stop ekapkgs-update.timer

# Disable timer
sudo systemctl disable ekapkgs-update.timer
```

## Web Dashboard Service

### Service File

```ini
# /etc/systemd/system/ekapkgs-update-web.service
[Unit]
Description=ekapkgs update web dashboard
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=ekapkgs
Group=ekapkgs
WorkingDirectory=/var/lib/ekapkgs

ExecStart=/usr/local/bin/ekapkgs-update-web \
  --database /var/lib/ekapkgs/db.sqlite3 \
  --host 127.0.0.1 \
  --port 3000

Restart=on-failure
RestartSec=10s

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=ekapkgs-web

# Hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/ekapkgs

[Install]
WantedBy=multi-user.target
```

### Enable and Start

```bash
# Install service
sudo cp ekapkgs-update-web.service /etc/systemd/system/
sudo systemctl daemon-reload

# Enable and start
sudo systemctl enable --now ekapkgs-update-web.service

# Check status
sudo systemctl status ekapkgs-update-web.service

# View logs
sudo journalctl -u ekapkgs-update-web.service -f
```

## Advanced Configuration

### Resource Limits

```ini
[Service]
# Memory limit
MemoryMax=16G
MemoryHigh=14G

# CPU limit (400% = 4 cores)
CPUQuota=400%

# I/O limits
IOWeight=500

# Process limits
TasksMax=200
```

### Security Hardening

```ini
[Service]
# Security settings
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ProtectKernelTunables=true
ProtectControlGroups=true
RestrictRealtime=true

# Filesystem access
ReadWritePaths=/var/lib/ekapkgs
ReadOnlyPaths=/nix/store

# Namespace isolation
PrivateDevices=true
ProtectHostname=true
```

### Notification on Failure

```ini
[Service]
# Send email on failure
OnFailure=status-email@%n.service
```

```ini
# /etc/systemd/system/status-email@.service
[Unit]
Description=Email Status for %i

[Service]
Type=oneshot
ExecStart=/usr/local/bin/send-status-email.sh %i
User=root
```

```bash
#!/bin/bash
# /usr/local/bin/send-status-email.sh
SERVICE=$1

STATUS=$(systemctl status "$SERVICE")

mail -s "Service Failed: $SERVICE" admin@example.com << EOF
Service $SERVICE has failed.

Status:
$STATUS

Logs:
$(journalctl -u "$SERVICE" -n 50)
EOF
```

## Monitoring

### Prometheus Exporter

```bash
# Install node_exporter with textfile collector
sudo systemctl enable --now node_exporter

# Create metrics script
cat > /usr/local/bin/ekapkgs-update-metrics.sh << 'EOF'
#!/bin/bash
TEXTFILE=/var/lib/node_exporter/ekapkgs-update.prom

# Query database
TOTAL=$(ekapkgs-update query --since-days 1 | grep -c "Package:")
SUCCESS=$(ekapkgs-update query --since-days 1 --status success | grep -c "Package:")
FAILED=$(ekapkgs-update query --since-days 1 --status failed | grep -c "Package:")

# Write metrics
cat > "$TEXTFILE" << METRICS
# HELP ekapkgs_updates_total Total updates in last 24h
# TYPE ekapkgs_updates_total gauge
ekapkgs_updates_total $TOTAL

# HELP ekapkgs_updates_success Successful updates in last 24h
# TYPE ekapkgs_updates_success gauge
ekapkgs_updates_success $SUCCESS

# HELP ekapkgs_updates_failed Failed updates in last 24h
# TYPE ekapkgs_updates_failed gauge
ekapkgs_updates_failed $FAILED
METRICS
EOF

chmod +x /usr/local/bin/ekapkgs-update-metrics.sh
```

```ini
# /etc/systemd/system/ekapkgs-update-metrics.timer
[Unit]
Description=Update ekapkgs metrics

[Timer]
OnCalendar=*:0/5  # Every 5 minutes

[Install]
WantedBy=timers.target
```

## Troubleshooting

### Service Won't Start

```bash
# Check service status
sudo systemctl status ekapkgs-update.service

# View detailed logs
sudo journalctl -u ekapkgs-update.service -xe

# Verify configuration
sudo systemd-analyze verify ekapkgs-update.service

# Test command manually
sudo -u ekapkgs /usr/local/bin/ekapkgs-update --help
```

### Timer Not Triggering

```bash
# Check timer status
sudo systemctl status ekapkgs-update.timer

# List all timers
sudo systemctl list-timers

# Verify timer calendar
systemd-analyze calendar "daily"
systemd-analyze calendar "Mon-Fri 02:00"
```

### Permission Issues

```bash
# Check ownership
ls -la /var/lib/ekapkgs

# Fix permissions
sudo chown -R ekapkgs:ekapkgs /var/lib/ekapkgs
sudo chmod 755 /var/lib/ekapkgs
```

## See Also

- [NixOS Module](./nixos-module.md) - Declarative NixOS configuration
- [Web Dashboard](./web-dashboard.md) - Web interface setup
- [CI/CD Integration](./use-cases/ci-cd.md) - Automation patterns
