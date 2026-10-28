# mkManyVariants Packages

Special handling for packages that use `mkManyVariants` to maintain multiple version streams simultaneously.

## Overview

mkManyVariants packages define multiple versions in a `variants.nix` file. ekapkgs-update understands this structure and can update individual variants or discover new ones.

## Strategy Inference

Update strategies are inferred from variant names if not explicitly specified:

| Variant name | Components | Inferred strategy | Behavior |
|-------------|-----------|------------------|----------|
| `v1` | 1 | `minor` | Update to latest `1.x.x` |
| `v1_2` | 2 | `patch` | Update to latest `1.2.x` |
| `v1_2_3` | 3 | pinned | Skipped (no updates) |

## Two Update Models

### In-Place Updates (Default)

Most mkManyVariants packages use 2-component variant keys (e.g., `v3_12` for Python 3.12). Each variant is updated in-place to the latest patch:

```nix
# variants.nix — before
v3_12 = {
  version = "3.12.4";
  hash = "sha256-old...";
};

# variants.nix — after update
v3_12 = {
  version = "3.12.7";
  hash = "sha256-new...";
};
```

This is the default behavior with `semver-strategy = "patch"`.

### Keep Mode (Version Retention)

Some packages need to retain every patch version as a separate entry. Set `keep = "patch"` for this:

```nix
# In generic.nix
passthru.ekapkgs-update = {
  github-repo = "golang/go";
  keep = "patch";
};
```

```nix
# variants.nix — each patch is its own entry
v1_26_8 = { version = "1.26.8"; src-hash = "sha256-..."; };
v1_26_9 = { version = "1.26.9"; src-hash = "sha256-..."; };  # added automatically
v1_26_10 = { version = "1.26.10"; src-hash = "sha256-..."; }; # added automatically
```

When new upstream patches are released, ekapkgs-update **adds** new entries by cloning the template from the last entry in the series. This preserves all fields (bootstrap references, cargo hashes, etc.).

See [Keep](../passthru-attributes/keep.md) for full details.

## CLI Usage

### Update Default Variant

```bash
ekapkgs-update update python3
```

### Update Specific Variant

```bash
ekapkgs-update update python3 --variant v3_12
```

### Update All Variants

```bash
ekapkgs-update update python3 --all-variants
```

This also discovers new variant series from upstream and fills in missing versions when `keep` is set.

## New Variant Discovery

When `--all-variants` is used, ekapkgs-update checks upstream for entirely new version series. For example, if Python 3.15.0 is released and no `v3_15` variant exists yet, the tool will add it automatically.

## Examples

### Language with In-Place Updates (Python)

```nix
# 2-component variants, updated in-place
passthru.ekapkgs-update.semver-strategy = "patch";
```

Variants `v3_12`, `v3_13`, etc. each get bumped to their latest patch.

### Language with Version Retention (Go)

```nix
# 3-component variants, new entries added
passthru.ekapkgs-update = {
  github-repo = "golang/go";
  version-regex = "^go(.+)$";
  keep = "patch";
};
```

Variants `v1_26_8`, `v1_26_9`, `v1_26_10` are all preserved as separate entries.

### Package with Custom Version Source (OpenSSL)

```nix
# Hosted on openssl.org, not GitHub — use update-script
passthru.ekapkgs-update = {
  update-script = writeScript "list-openssl-releases" ''
    ${curl}/bin/curl -s https://www.openssl.org/source/ \
      | ${pcregrep}/bin/pcregrep -o1 'openssl-([0-9]+\.[0-9]+\.[0-9]+)\.tar' \
      | sort -uV
  '';
  semver-strategy = "patch";
};
```

The script lists versions; ekapkgs-update applies `semver-strategy` filtering and updates the variant.
