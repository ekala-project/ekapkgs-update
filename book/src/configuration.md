# Configuration

> **Note:** This chapter is under construction.

ekapkgs-update can be configured through:

1. **Passthru Attributes** - Per-package configuration (see [Passthru Attributes](./passthru-attributes.md))
2. **CLI Arguments** - Command-line flags
3. **Environment Variables** - GitHub/GitLab tokens, logging

## Environment Variables

```bash
export GITHUB_TOKEN="ghp_..."
export GITLAB_TOKEN="glpat-..."
export RUST_LOG="debug"
```

## Per-Package Configuration

See [Passthru Attributes](./passthru-attributes.md) for detailed documentation.
