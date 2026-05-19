# Troubleshooting

Common issues and solutions when using ekapkgs-update.

## Installation Issues

### Nix Flakes Not Enabled

**Error:**
```
error: experimental Nix feature 'flakes' is disabled
```

**Solution:**
```bash
# Enable flakes in ~/.config/nix/nix.conf
mkdir -p ~/.config/nix
echo "experimental-features = nix-command flakes" >> ~/.config/nix/nix.conf

# Or set environment variable
export NIX_CONFIG="experimental-features = nix-command flakes"
```

### Command Not Found

**Error:**
```bash
$ ekapkgs-update
command not found: ekapkgs-update
```

**Solution:**
```bash
# Check if installed
nix profile list | grep ekapkgs-update

# If not installed
nix profile install github:ekapkgs/ekapkgs-update

# Add to PATH
export PATH="$HOME/.nix-profile/bin:$PATH"

# Or use nix run
nix run github:ekapkgs/ekapkgs-update -- --help
```

## Update Failures

### Hash Mismatch

**Error:**
```
Error: hash mismatch in fixed-output derivation
  specified: sha256-AAAA...
  got:        sha256-BBBB...
```

**Cause:** Upstream changed release tarball or different source than expected.

**Solution:**
```bash
# ekapkgs-update automatically fixes this
# Just re-run the update
ekapkgs-update update mypackage

# If still fails, verify source manually
curl -L "https://github.com/owner/repo/archive/v2.5.0.tar.gz" | sha256sum
```

### Build Failure

**Error:**
```
Error: builder for '/nix/store/...-mypackage-2.5.0.drv' failed with exit code 2
```

**Debug steps:**
```bash
# 1. View logs
ekapkgs-update log mypackage

# 2. Inspect details
ekapkgs-update inspect mypackage

# 3. Check worktree if preserved
ekapkgs-update worktrees show mypackage

# 4. Try building manually
cd /tmp/ekapkgs-update-worktrees/mypackage
nix-build -A mypackage
```

**Common causes:**
- Missing dependencies
- Outdated patches
- Build system changes
- Incompatible flags

See [Debugging Guide](../use-cases/debugging.md) for detailed solutions.

### Patch Application Failure

**Error:**
```
patching file src/main.c
Hunk #1 FAILED at 45.
```

**Solution:**
```bash
# Option 1: Remove outdated patch
cd /tmp/ekapkgs-update-worktrees/mypackage
# Edit default.nix to remove patch
git diff > /tmp/remove-patch.patch
ekapkgs-update apply mypackage --patch /tmp/remove-patch.patch --resume

# Option 2: Update patch
# Manually update patch to match new code
# Then retry

# Option 3: Check if patch was applied upstream
# Remove if no longer needed
```

### Test Failure

**Error:**
```
Error: passthru.tests.pytest failed
```

**Solution:**
```bash
# View test output
ekapkgs-update log mypackage | grep -A 50 "checkPhase"

# Disable tests temporarily
cd /tmp/ekapkgs-update-worktrees/mypackage
# Edit default.nix: doCheck = false;
git diff > /tmp/disable-tests.patch
ekapkgs-update apply mypackage --patch /tmp/disable-tests.patch --resume

# Or disable specific tests
# checkPhase = "pytest --deselect tests/test_flaky.py";
```

### Version Not Found

**Error:**
```
Error: No compatible version found matching strategy 'minor'
```

**Solutions:**
```bash
# 1. Try different strategy
ekapkgs-update update mypackage --semver latest

# 2. Check available versions
curl -s https://api.github.com/repos/owner/repo/releases | jq '.[].tag_name'

# 3. Specify explicit version
ekapkgs-update update mypackage --version 2.5.0

# 4. Check version regex
ekapkgs-update update mypackage --version-regex 'release-(.*)'
```

## Rate Limiting

### GitHub Rate Limit

**Error:**
```
Error: API rate limit exceeded
```

**Solution:**
```bash
# Check current rate limit
curl -H "Authorization: token $GITHUB_TOKEN" https://api.github.com/rate_limit

# Without token: 60 requests/hour
# With token: 5000 requests/hour

# Set token
export GITHUB_TOKEN="ghp_xxxxxxxxxxxxx"

# Verify it works
curl -H "Authorization: token $GITHUB_TOKEN" https://api.github.com/user
```

**Generate token:**
1. Visit https://github.com/settings/tokens
2. Generate new token (classic)
3. Select `repo` scope
4. Copy token

### GitLab Rate Limit

**Error:**
```
Error: GitLab API rate limit exceeded
```

**Solution:**
```bash
# Set GitLab token
export GITLAB_TOKEN="glpat-xxxxxxxxxxxxx"

# Verify
curl -H "PRIVATE-TOKEN: $GITLAB_TOKEN" https://gitlab.com/api/v4/user
```

## Database Issues

### Database Locked

**Error:**
```
Error: database is locked
```

**Cause:** Another instance is accessing the database.

**Solution:**
```bash
# Check for running instances
ps aux | grep ekapkgs-update

# Kill if stuck
pkill -9 ekapkgs-update

# Or use different database
ekapkgs-update run --database /tmp/test-db.sqlite3
```

### Database Corruption

**Error:**
```
Error: database disk image is malformed
```

**Solution:**
```bash
# Backup database
cp ~/.cache/ekapkgs-update/db.sqlite3{,.backup}

# Try to recover
sqlite3 ~/.cache/ekapkgs-update/db.sqlite3 ".recover" | \
  sqlite3 ~/.cache/ekapkgs-update/db-recovered.sqlite3

# If recovery fails, start fresh
mv ~/.cache/ekapkgs-update/db.sqlite3{,.corrupted}
# Database will be recreated on next run
```

### Database Not Found

**Error:**
```
Error: unable to open database file
```

**Solution:**
```bash
# Create directory
mkdir -p ~/.cache/ekapkgs-update

# Database will be created automatically
ekapkgs-update run

# Or specify location
ekapkgs-update run --database /var/lib/ekapkgs-update/db.sqlite3
```

## Git Issues

### Not a Git Repository

**Error:**
```
Error: not a git repository
```

**Solution:**
```bash
# Initialize git repository
git init

# Or run from within repository
cd /path/to/repo
ekapkgs-update run
```

### No Remote Configured

**Error:**
```
Error: no remote 'origin' found
```

**Solution:**
```bash
# Add remote
git remote add origin https://github.com/user/repo.git

# Or specify fork
ekapkgs-update run --fork my-fork

# For PR creation, also need upstream
git remote add upstream https://github.com/nixpkgs/nixpkgs.git
ekapkgs-update run --upstream upstream --fork origin
```

### Dirty Working Tree

**Error:**
```
Error: working tree has uncommitted changes
```

**Solution:**
```bash
# Commit changes
git add .
git commit -m "WIP"

# Or stash
git stash

# Or reset
git reset --hard HEAD
```

### Permission Denied (GitHub)

**Error:**
```
Error: permission denied (publickey)
```

**Solution:**
```bash
# Use HTTPS instead of SSH
git remote set-url origin https://github.com/user/repo.git

# Or set up SSH key
ssh-keygen -t ed25519 -C "your_email@example.com"
cat ~/.ssh/id_ed25519.pub
# Add to GitHub: Settings -> SSH Keys
```

## Network Issues

### Connection Timeout

**Error:**
```
Error: operation timed out
```

**Solutions:**
```bash
# 1. Check network
ping github.com

# 2. Check proxy settings
export HTTP_PROXY=http://proxy.example.com:8080
export HTTPS_PROXY=http://proxy.example.com:8080

# 3. Check firewall
sudo iptables -L

# 4. Retry with timeout
timeout 300 ekapkgs-update update mypackage
```

### SSL Certificate Verification Failed

**Error:**
```
Error: SSL certificate problem: unable to get local issuer certificate
```

**Solutions:**
```bash
# Update CA certificates (Debian/Ubuntu)
sudo apt-get update
sudo apt-get install ca-certificates

# Update CA certificates (macOS)
brew install ca-certificates

# As last resort (not recommended for production)
export NIX_SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
```

## Permission Issues

### Permission Denied (File)

**Error:**
```
Error: Permission denied (os error 13)
```

**Solutions:**
```bash
# Check file permissions
ls -la /path/to/file

# Fix ownership
sudo chown -R $USER:$USER /path/to/repo

# Fix permissions
chmod 644 /path/to/file

# For directories
chmod 755 /path/to/directory
```

### Cannot Create Directory

**Error:**
```
Error: cannot create directory: Permission denied
```

**Solutions:**
```bash
# Create directory with proper permissions
mkdir -p ~/.cache/ekapkgs-update
chmod 755 ~/.cache/ekapkgs-update

# Or use custom location
ekapkgs-update run --database /tmp/db.sqlite3
```

## Resource Issues

### Out of Memory

**Error:**
```
Error: cannot allocate memory
```

**Solutions:**
```bash
# 1. Reduce concurrency
ekapkgs-update run --concurrent-updates 2

# 2. Close other applications

# 3. Add swap space
sudo fallocate -l 4G /swapfile
sudo chmod 600 /swapfile
sudo mkswap /swapfile
sudo swapon /swapfile

# 4. Use chunked updates
ekapkgs-update run --max-rebuilds 10
```

### Out of Disk Space

**Error:**
```
Error: No space left on device
```

**Solutions:**
```bash
# 1. Clean Nix store
nix-collect-garbage -d

# 2. Clean old worktrees
ekapkgs-update worktrees clean --older-than 1

# 3. Check disk usage
df -h
du -sh ~/.cache/ekapkgs-update/*

# 4. Clean old generations
nix-collect-garbage --delete-older-than 7d
```

### Too Many Open Files

**Error:**
```
Error: Too many open files (os error 24)
```

**Solution:**
```bash
# Increase file limit
ulimit -n 4096

# Or permanently (add to ~/.bashrc)
echo "ulimit -n 4096" >> ~/.bashrc

# Check current limit
ulimit -n
```

## Performance Issues

### Slow Updates

**Symptoms:** Updates taking longer than expected.

**Solutions:**
```bash
# 1. Increase concurrency
ekapkgs-update run --concurrent-updates 16

# 2. Skip optional checks
ekapkgs-update run \
  --skip-cve \
  --skip-repology \
  --skip-directory-diff

# 3. Use Cachix
ekapkgs-update run --cachix-cache my-cache

# 4. Check system resources
top
htop
```

### High CPU Usage

**Solution:**
```bash
# Limit CPU usage
ekapkgs-update run --concurrent-updates 4

# Or use nice/ionice
nice -n 19 ekapkgs-update run
ionice -c 3 ekapkgs-update run
```

## Web Dashboard Issues

### Dashboard Won't Start

**Error:**
```
Error: Address already in use
```

**Solution:**
```bash
# Check what's using port
lsof -i :3000
netstat -tulpn | grep 3000

# Use different port
ekapkgs-update-web --port 8080

# Or kill the process
kill $(lsof -t -i:3000)
```

### Cannot Connect to Dashboard

**Solutions:**
```bash
# 1. Check dashboard is running
ps aux | grep ekapkgs-update-web

# 2. Check port binding
ekapkgs-update-web --host 0.0.0.0 --port 3000

# 3. Check firewall
sudo ufw allow 3000

# 4. Test locally
curl http://localhost:3000
```

## Getting Help

### Enable Debug Logging

```bash
export RUST_LOG=debug
ekapkgs-update run

# Or very verbose
export RUST_LOG=trace
ekapkgs-update update mypackage
```

### Gather Debug Information

```bash
# System info
uname -a
nix --version

# ekapkgs-update version
ekapkgs-update --version

# Check configuration
cat ~/.config/nix/nix.conf

# Recent logs
ekapkgs-update query --since-days 1 --status failed

# Database stats
sqlite3 ~/.cache/ekapkgs-update/db.sqlite3 \
  "SELECT COUNT(*) FROM update_attempts;"
```

### Report Issues

When reporting issues, include:
1. ekapkgs-update version
2. Command that failed
3. Full error message
4. Debug logs (`RUST_LOG=debug`)
5. Minimal reproduction steps

## See Also

- [Debugging Guide](../use-cases/debugging.md) - Detailed debugging workflows
- [FAQ](./faq.md) - Common questions
- [Environment Variables](./environment.md) - Configuration reference
