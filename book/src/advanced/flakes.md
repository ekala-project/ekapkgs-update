# Flake Updates

> **Note:** This chapter is under construction.

ekapkgs-update supports updating packages exposed by flakes.

## Basic Usage

```bash
ekapkgs-update update --flake .#mypackage
```

## Limitations

Flake packages do not currently support `passthru.ekapkgs-update` attributes.
