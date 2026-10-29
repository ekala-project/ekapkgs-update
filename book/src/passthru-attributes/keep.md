# Keep

The `keep` attribute controls version retention for `mkManyVariants` packages. It tells ekapkgs-update that each version at a given granularity should be preserved as its own entry in `variants.nix`, rather than being updated in-place.

## Syntax

```nix
passthru.ekapkgs-update.keep = "patch";  # or "minor" | "major"
```

**Type:** `string` (enum)
**Default:** `null` (no keep mode — variants are updated in-place)
**Implemented:** ✅ Yes

## How It Works

Without `keep`, ekapkgs-update treats each variant as a mutable entry that gets bumped to the latest version in its series. For example, a `v1_26` variant tracking Go 1.26.8 would be updated to 1.26.10 by rewriting the version and hash in-place.

With `keep = "patch"`, each patch version gets its own immutable entry. Instead of modifying `v1_26_8`, the tool **adds** `v1_26_9` and `v1_26_10` as new entries — cloning the template from the last entry in the series so that all fields (bootstrap references, cargo hashes, helper functions, etc.) are preserved.

### Keep Levels

| Value | Variant granularity | Example variants |
|-------|-------------------|-----------------|
| `"patch"` | One entry per patch version | `v1_26_8`, `v1_26_9`, `v1_26_10` |
| `"minor"` | One entry per minor version | `v1_24`, `v1_25`, `v1_26` |
| `"major"` | One entry per major version | `v1`, `v2`, `v3` |

## When to Use `keep`

Use `keep` for packages where users need to pin to specific patch versions and all versions must remain available simultaneously:

- **Language toolchains** — Go, Ruby, where downstream projects may require exact compiler versions
- **Runtimes** — where different applications depend on different patch versions
- **Security-sensitive packages** — where each patch may address specific CVEs and users need to choose

Do **not** use `keep` when:
- Only the latest patch matters (use `semver-strategy = "patch"` instead)
- The variant structure uses 2-component keys like `v1_26` (these are already updated in-place correctly)

## Examples

### Go Compiler

Go uses per-patch entries (`v1_26_8`, `v1_26_9`, etc.) because different projects may require specific Go versions:

```nix
passthru.ekapkgs-update = {
  github-repo = "golang/go";
  version-regex = "^go(.+)$";
  keep = "patch";
};
```

When a new Go patch is released (e.g., 1.27.4), ekapkgs-update adds a `v1_27_4` entry by cloning the template from `v1_27_3`, then discovers the correct source hash and verifies the build.

### Ruby

Ruby similarly retains all patch versions:

```nix
passthru.ekapkgs-update = {
  semver-strategy = "patch";
  github-repo = "ruby/ruby";
  keep = "patch";
};
```

## Template Cloning

When `keep` adds a new variant, it **clones** the last existing entry in the same series rather than creating a minimal template. This means:

- All additional attributes (e.g., `cargoHash`, `bootstrapGo`, `iana-patch`) are preserved
- Helper function calls (e.g., `rubyVersion "4" "0" "7" ""`) are adapted to the new version
- Only the version string and source hash are changed

This ensures new entries are structurally identical to their predecessors, avoiding build failures from missing fields.

## Interaction with Other Attributes

### `semver-strategy`

`keep` and `semver-strategy` are complementary:

- `semver-strategy` controls **which** upstream versions are acceptable (e.g., only patch versions within a series)
- `keep` controls **how** updates are applied (add new entries vs. update in-place)

### `github-repo`

`keep` mode requires the tool to fetch upstream releases to find missing versions. If the package's source URL isn't from a recognized platform (GitHub, GitLab, etc.), you must also set `github-repo`:

```nix
passthru.ekapkgs-update = {
  github-repo = "golang/go";  # Required: go.dev URLs aren't auto-detected
  keep = "patch";
};
```

### `version-regex`

If upstream tags use a non-standard format, combine with `version-regex`:

```nix
passthru.ekapkgs-update = {
  github-repo = "golang/go";
  version-regex = "^go(.+)$";  # Tags are "go1.27.3", not "v1.27.3"
  keep = "patch";
};
```

## CLI Usage

The `keep` attribute is read automatically during `--all-variants` updates:

```bash
# This will add missing patch versions for all series
ekapkgs-update update go --file default.nix --all-variants --commit
```

In daemon mode (`ekapkgs-update run`), `keep` is respected automatically when processing mkManyVariants packages.

## Troubleshooting

### New Entries Have Wrong Fields

If added entries are missing fields like `cargoHash` or `bootstrapGo`, ensure the last existing entry in the series is complete and well-formed. The tool clones from the last entry.

### No Versions Added

1. Check that `keep` is set correctly:
   ```bash
   nix-instantiate --eval -E 'with import ./default.nix {}; go.passthru.ekapkgs-update.keep or "NONE"' --raw
   ```

2. Verify upstream has newer versions than what's in `variants.nix`

3. Check that `github-repo` is set if the source URL isn't auto-detectable

### Version Regex Not Matching

If versions aren't being extracted from tags, test your regex:
```bash
echo "go1.27.3" | grep -oP '^go(.+)$'
```

## Related

- [Semver Strategy](./semver-strategy.md) — Controls acceptable version ranges
- [Version Regex](./version-regex.md) — Custom tag parsing
- [mkManyVariants Packages](../advanced/many-variants.md) — General variant handling
