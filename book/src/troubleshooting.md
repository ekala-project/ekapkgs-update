# Troubleshooting

> **Note:** This chapter is under construction.

## Common Issues

### Rate Limiting

If you see "rate limit exceeded" errors:

```bash
# Set GitHub token
export GITHUB_TOKEN="ghp_..."
```

### Debug Logging

Enable detailed logging:

```bash
RUST_LOG=debug ekapkgs-update update mypackage
```

### Passthru Attributes Not Working

Verify the attribute exists:

```bash
nix-instantiate --eval -E 'with import ./default.nix {}; myapp.passthru.ekapkgs-update'
```

See individual passthru attribute pages for specific troubleshooting:
- [Skip](./passthru-attributes/skip.md#troubleshooting)
- [Semver Strategy](./passthru-attributes/semver-strategy.md#troubleshooting)
- [Include Prereleases](./passthru-attributes/include-prereleases.md#troubleshooting)
- [Version Regex](./passthru-attributes/version-regex.md#troubleshooting)
