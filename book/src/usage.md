# Usage Guide

> **Note:** This chapter is under construction.

`ekapkgs-update` has two primary modes of operation:

## Manual Updates

Update specific packages on-demand. See [Manual Updates](./usage/manual-updates.md) for details.

```bash
ekapkgs-update update mypackage
```

## Daemon Mode

Continuously monitor and update packages automatically. See [Daemon Mode](./usage/daemon-mode.md) for details.

```bash
ekapkgs-update run --file ./default.nix
```

## Per-Package Configuration

Use [Passthru Attributes](./passthru-attributes.md) to configure update behavior for individual packages.
