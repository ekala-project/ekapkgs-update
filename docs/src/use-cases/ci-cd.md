# CI/CD Integration

Integrate ekapkgs-update into continuous integration and deployment pipelines.

## GitHub Actions

### Daily Update Workflow

```yaml
# .github/workflows/update-packages.yml
name: Update Packages

on:
  schedule:
    - cron: '0 2 * * *'  # 2 AM daily
  workflow_dispatch:      # Manual trigger

jobs:
  update:
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write

    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - uses: DeterminateSystems/nix-installer-action@main

      - uses: DeterminateSystems/magic-nix-cache-action@main

      - name: Setup ekapkgs-update
        run: |
          nix profile install .#ekapkgs-update

      - name: Run updates
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          ekapkgs-update run \
            --skip-unstable \
            --run-passthru-tests \
            --analyze-rebuilds \
            --max-rebuilds 100 \
            --concurrent-updates 4

      - name: Report status
        if: always()
        run: |
          ekapkgs-update status
          ekapkgs-update query --since-days 1 --group-by-error

      - name: Upload failure artifacts
        if: failure()
        uses: actions/upload-artifact@v4
        with:
          name: update-failures
          path: |
            /tmp/ekapkgs-update-worktrees/
            ~/.cache/ekapkgs-update/db.sqlite3
```

### Dry-Run on PRs

```yaml
# .github/workflows/test-updates.yml
name: Test Package Updates

on:
  pull_request:
    paths:
      - 'pkgs/**'

jobs:
  test-updates:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: DeterminateSystems/nix-installer-action@main

      - name: Check updatability
        run: |
          nix profile install .#ekapkgs-update
          ekapkgs-update run --dry-run
```

### Weekly Report

```yaml
# .github/workflows/update-report.yml
name: Weekly Update Report

on:
  schedule:
    - cron: '0 9 * * 1'  # Monday 9 AM

jobs:
  report:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: DeterminateSystems/nix-installer-action@main

      - name: Generate report
        run: |
          nix profile install .#ekapkgs-update

          echo "# Package Update Report" > report.md
          echo "" >> report.md
          echo "## Available Updates" >> report.md
          ekapkgs-update run --dry-run >> report.md
          echo "" >> report.md
          echo "## Recent Failures" >> report.md
          ekapkgs-update query --since-days 7 --group-by-error >> report.md

      - name: Create Issue
        uses: peter-evans/create-issue-from-file@v5
        with:
          title: 'Weekly Package Update Report'
          content-filepath: report.md
          labels: 'automated, updates'
```

## GitLab CI

### Daily Pipeline

```yaml
# .gitlab-ci.yml
update-packages:
  image: nixos/nix:latest

  before_script:
    - nix profile install .#ekapkgs-update

  script:
    - |
      ekapkgs-update run \
        --skip-unstable \
        --max-rebuilds 100 \
        --database /cache/ekapkgs-update/db.sqlite3

  after_script:
    - ekapkgs-update status --database /cache/ekapkgs-update/db.sqlite3
    - ekapkgs-update query --since-days 1 --group-by-error --database /cache/ekapkgs-update/db.sqlite3

  cache:
    paths:
      - /cache/ekapkgs-update/

  artifacts:
    when: on_failure
    paths:
      - /tmp/ekapkgs-update-worktrees/

  only:
    - schedules

  allow_failure: true
```

## Jenkins

### Pipeline

```groovy
// Jenkinsfile
pipeline {
    agent { label 'nix' }

    triggers {
        cron('H 2 * * *')  // Daily at 2 AM
    }

    environment {
        GITHUB_TOKEN = credentials('github-token')
        CACHIX_AUTH_TOKEN = credentials('cachix-token')
        DATABASE_PATH = '/var/lib/jenkins/ekapkgs-update/db.sqlite3'
    }

    stages {
        stage('Setup') {
            steps {
                sh 'nix profile install .#ekapkgs-update'
            }
        }

        stage('Update') {
            steps {
                sh '''
                    ekapkgs-update run \
                        --database ${DATABASE_PATH} \
                        --skip-unstable \
                        --run-passthru-tests \
                        --max-rebuilds 100 \
                        --cachix-cache my-cache \
                        --concurrent-updates 8
                '''
            }
        }

        stage('Report') {
            steps {
                sh '''
                    ekapkgs-update status --database ${DATABASE_PATH}
                    ekapkgs-update query --since-days 1 --group-by-error --database ${DATABASE_PATH}
                '''
            }
        }
    }

    post {
        always {
            archiveArtifacts artifacts: 'logs/*.log', allowEmptyArchive: true
        }
        failure {
            emailext (
                subject: "Package Updates Failed: ${env.JOB_NAME} ${env.BUILD_NUMBER}",
                body: '''${BUILD_LOG}''',
                to: 'team@example.com'
            )
        }
    }
}
```

## Docker

### Containerized Updates

```dockerfile
# Dockerfile
FROM nixos/nix:latest

# Install ekapkgs-update
COPY . /workspace
WORKDIR /workspace
RUN nix profile install .#ekapkgs-update

# Create volumes for persistence
VOLUME ["/cache", "/worktrees"]

# Default command
ENTRYPOINT ["ekapkgs-update"]
CMD ["run", "--database", "/cache/db.sqlite3", "--preserve-failures"]
```

**Usage:**
```bash
# Build
docker build -t ekapkgs-update .

# Run
docker run \
  -v $(pwd):/workspace \
  -v ekapkgs-cache:/cache \
  -v ekapkgs-worktrees:/worktrees \
  -e GITHUB_TOKEN="$GITHUB_TOKEN" \
  ekapkgs-update run --skip-unstable
```

## Systemd Timer

### Service File

```ini
# /etc/systemd/system/ekapkgs-update.service
[Unit]
Description=Update ekapkgs packages
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
User=ekapkgs
WorkingDirectory=/var/lib/ekapkgs
Environment="GITHUB_TOKEN=ghp_..."
Environment="CACHIX_AUTH_TOKEN=..."
ExecStart=/usr/bin/ekapkgs-update run \
  --file /var/lib/ekapkgs/pkgs/default.nix \
  --database /var/lib/ekapkgs/db.sqlite3 \
  --skip-unstable \
  --max-rebuilds 100 \
  --cachix-cache production
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

### Timer File

```ini
# /etc/systemd/system/ekapkgs-update.timer
[Unit]
Description=Daily package updates
Requires=ekapkgs-update.service

[Timer]
OnCalendar=daily
OnCalendar=02:00
Persistent=true

[Install]
WantedBy=timers.target
```

**Enable:**
```bash
sudo systemctl enable ekapkgs-update.timer
sudo systemctl start ekapkgs-update.timer

# Check status
sudo systemctl status ekapkgs-update.timer
sudo journalctl -u ekapkgs-update.service -f
```

## Monitoring and Alerts

### Prometheus Metrics

```bash
#!/bin/bash
# export-metrics.sh

DB="/var/lib/ekapkgs-update/db.sqlite3"

# Total updates (last 24h)
TOTAL=$(ekapkgs-update query --database "$DB" --since-days 1 | grep -c "Package:")
echo "ekapkgs_updates_total{period=\"24h\"} $TOTAL"

# Successful updates
SUCCESS=$(ekapkgs-update query --database "$DB" --since-days 1 --status success | grep -c "Package:")
echo "ekapkgs_updates_success{period=\"24h\"} $SUCCESS"

# Failed updates
FAILED=$(ekapkgs-update query --database "$DB" --since-days 1 --status failed | grep -c "Package:")
echo "ekapkgs_updates_failed{period=\"24h\"} $FAILED"

# Success rate
if [ "$TOTAL" -gt 0 ]; then
    RATE=$(echo "scale=2; $SUCCESS / $TOTAL * 100" | bc)
    echo "ekapkgs_success_rate{period=\"24h\"} $RATE"
fi
```

### Slack Notifications

```bash
#!/bin/bash
# notify-slack.sh

WEBHOOK_URL="https://hooks.slack.com/services/..."
STATUS=$(ekapkgs-update status)

# Parse results
TOTAL=$(echo "$STATUS" | grep "Total:" | awk '{print $2}')
SUCCESS=$(echo "$STATUS" | grep "Success:" | awk '{print $2}')
FAILED=$(echo "$STATUS" | grep "Failed:" | awk '{print $2}')

# Send notification
curl -X POST "$WEBHOOK_URL" \
  -H 'Content-Type: application/json' \
  -d @- << EOF
{
  "text": "Package Update Complete",
  "attachments": [{
    "color": "$([ "$FAILED" -eq 0 ] && echo 'good' || echo 'warning')",
    "fields": [
      {"title": "Total", "value": "$TOTAL", "short": true},
      {"title": "Success", "value": "$SUCCESS", "short": true},
      {"title": "Failed", "value": "$FAILED", "short": true}
    ]
  }]
}
EOF
```

## Best Practices

### Environment Variables

```bash
# Never commit tokens
export GITHUB_TOKEN="ghp_..."
export CACHIX_AUTH_TOKEN="..."

# Or use secret management
GITHUB_TOKEN=$(vault kv get -field=token secret/github)
```

### Error Handling

```bash
#!/bin/bash
set -euo pipefail

# Run updates
if ! ekapkgs-update run --preserve-failures; then
    echo "Updates failed, generating report..."
    ekapkgs-update query --since-days 1 --status failed > failures.txt
    # Send alert
    mail -s "Update Failures" team@example.com < failures.txt
    exit 1
fi

# Clean up on success
ekapkgs-update worktrees clean --older-than 7
```

### Resource Management

```bash
# Limit concurrent updates based on CI resources
if [ -n "$CI" ]; then
    CONCURRENT=4
else
    CONCURRENT=8
fi

ekapkgs-update run --concurrent-updates "$CONCURRENT"
```

## See Also

- [NixOS Module](../nixos-module.md) - Declarative configuration
- [Systemd Service](../systemd.md) - Manual service setup
- [Batch Updates](./batch-updates.md) - Workflow patterns
