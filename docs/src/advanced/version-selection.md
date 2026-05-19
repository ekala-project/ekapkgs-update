# Version Selection and Semver Strategies

ekapkgs-update uses semantic versioning strategies to intelligently select which upstream versions to propose for updates. This prevents jumping to major versions when a patch is appropriate, and supports variant-based version pinning.

## Overview

The system supports four update strategies:

| Strategy | Behavior | Use Case |
|----------|----------|----------|
| `latest` | Always propose the newest upstream version | Breaking-change tolerant packages |
| `major` | Update within major version (X.y.z) | Special cases, rarely used |
| `minor` | Update within minor version (X.Y.z) | Most packages, preferred default |
| `patch` | Update within patch version (X.Y.z) | Highly stable packages, critical dependencies |

## Semantic Versioning Basics

A semantic version has three components: **MAJOR.MINOR.PATCH**

```
2.31.5
│  │  │
│  │  └─ PATCH: Bug fixes, no new features
│  └──── MINOR: New features, backward compatible
└─────── MAJOR: Breaking changes, API incompatible
```

### Version Ranges

Each strategy defines which versions are considered compatible:

**Patch** (latest 2.31.z):
```
2.30.0  ← not included
2.31.0  ← included
2.31.5  ← included (within patch range)
2.32.0  ← not included (new minor version)
3.0.0   ← not included (new major version)
```

**Minor** (latest 2.Y.z):
```
1.99.0  ← not included
2.0.0   ← included
2.31.5  ← included
2.999.0 ← included (within minor range)
3.0.0   ← not included (new major version)
```

**Major** (latest X.y.z):
```
1.0.0   ← included
2.31.5  ← included
3.0.0   ← included
4.0.0   ← included
```

**Latest**:
```
All versions are considered.
Always proposes the absolute newest.
```

## Strategy Selection

### Configuration

Strategies are specified in package configuration:

```nix
passthru.ekapkgs-update = {
  enable = true;
  strategy = "minor";  # Default: "minor"
};
```

### Auto-Detection from Variants

For packages using `mkManyVariants` with version-based variants, the strategy is automatically inferred:

```nix
mkManyVariants {
  versions = {
    v1_2 = { ... };    # Inferred: patch (keep 1.2.z)
    v1 = { ... };      # Inferred: minor (keep 1.y.z)
    v1_2_3 = { ... };  # Inferred: pinned, no auto-update
  };
};
```

### Inference Rules

```
Variant:        Inferred Strategy:        Rationale:
─────────────── ─────────────────────── ──────────────────────────
v1              minor                   1.y.z updates
v1_2            patch                   1.2.z updates
v1_2_3          (pinned, no auto-update) Exact version pinning
latest          (no auto-update)         Handled manually
default         (depends on version)     Normal strategy applies
```

#### Examples

**v1_2** variant → Patch strategy:
```nix
mkManyVariants {
  versions.v1_2 = {
    src = fetchFromGitHub {
      rev = "v1.2.999";  # Any 1.2.z is acceptable
    };
  };
};
```
If current version is `1.2.8` and upstream has `1.2.9`, `1.3.0`, and `2.0.0`:
- Proposer selects: `1.2.9` (latest patch)
- Ignores: `1.3.0` (minor), `2.0.0` (major)

**v1** variant → Minor strategy:
```nix
mkManyVariants {
  versions.v1 = {
    src = fetchFromGitHub {
      rev = "v1.999.0";  # Any 1.y.z is acceptable
    };
  };
};
```
If current version is `1.8.3` and upstream has `1.9.0`, `2.0.0`:
- Proposer selects: `1.9.0` (latest minor)
- Ignores: `2.0.0` (major)

**v1_2_3** variant → Pinned (no auto-update):
```nix
mkManyVariants {
  versions.v1_2_3 = {
    # This version is pinned. Updates must be manual.
  };
};
```

## Version Matching Logic

### Parsing Versions

The system parses semantic versions using standard semver rules:

```rust
fn parse_version(version_str: &str) -> Option<(u32, u32, u32)> {
    // Parse "2.31.5" → (2, 31, 5)
    // Parse "2.31" → (2, 31, 0)
    // Parse "v2.31.5" → (2, 31, 5) (strips 'v' prefix)
    // Returns None for non-semver versions
}
```

### Compatibility Check

Given a strategy and version range, compatibility is determined:

```rust
fn is_compatible(
    current: (u32, u32, u32),
    candidate: (u32, u32, u32),
    strategy: Strategy,
) -> bool {
    match strategy {
        Patch =>
            candidate.major == current.major &&
            candidate.minor == current.minor &&
            candidate.patch >= current.patch,

        Minor =>
            candidate.major == current.major &&
            (candidate.minor > current.minor ||
             (candidate.minor == current.minor &&
              candidate.patch >= current.patch)),

        Major | Latest =>
            candidate > current,
    }
}
```

### Examples

**Strategy: Patch, Current: 2.31.5**

| Candidate | Compatible? | Reason |
|-----------|------------|--------|
| 2.31.6 | ✓ | Same major.minor, patch increased |
| 2.31.5 | ✗ | Same version (no upgrade needed) |
| 2.31.4 | ✗ | Patch decreased (downgrade) |
| 2.32.0 | ✗ | Minor increased (out of patch range) |
| 3.0.0 | ✗ | Major increased (out of patch range) |

**Strategy: Minor, Current: 2.8.3**

| Candidate | Compatible? | Reason |
|-----------|------------|--------|
| 2.9.0 | ✓ | Same major, minor increased |
| 2.8.4 | ✓ | Same major.minor, patch increased |
| 2.8.3 | ✗ | Same version (no upgrade needed) |
| 3.0.0 | ✗ | Major increased (out of minor range) |

## Pre-Release and Build Metadata

The system handles pre-release versions (alpha, beta, rc) carefully:

```
2.0.0-alpha     Pre-release (lower priority)
2.0.0-beta.1    Pre-release with metadata
2.0.0-rc1       Release candidate
2.0.0           Stable release (preferred)
```

**Default Behavior**: Pre-release versions are deprioritized in favor of stable releases:
- If both `2.0.0` (stable) and `2.0.0-rc1` (pre-release) are available, selects `2.0.0`
- If only pre-releases are available, selects the latest pre-release

### Override

To prefer pre-releases or accept any version:

```nix
passthru.ekapkgs-update = {
  enable = true;
  allow-prerelease = true;  # Accept alpha, beta, rc versions
};
```

## Non-Semver Versions

Some packages don't follow semantic versioning:

```
2024.05.15      Calendar versioning (date-based)
1.2.3+git.abc   Git revision suffix
1.2.3_custom    Vendor-specific suffix
v1.2.3          Version with prefix
```

**Handling**: The system attempts to extract semantic components when possible, but falls back to string comparison:

```
"2024.05.15" parsed as (2024, 5, 15) for strategy matching
If parsing fails, falls back to "latest" strategy
```

## Version Range Queries

The system queries upstream repositories to find available versions:

### GitHub

Fetches all releases (semantic or otherwise) and filters by strategy:

```bash
# Hypothetical upstream: https://github.com/psf/requests
releases:
  - v2.32.1  ← selected (latest patch for 2.32)
  - v2.32.0
  - v2.31.0
  - v2.0.0
```

### PyPI

Queries all available versions:

```bash
# Hypothetical upstream: https://pypi.org/project/requests/
versions:
  - 2.32.1   ← selected (latest patch for 2.32)
  - 2.32.0
  - 2.31.0
  - 2.0.0
  - 2.0.0.dev1
```

### GitLab

Similar to GitHub, uses release/tag information:

```bash
# Hypothetical upstream: https://gitlab.com/example/project
tags:
  - v2.32.1  ← selected
  - v2.32.0
  - v2.31.0
```

## Practical Examples

### Example 1: Patch Updates Only

A critical dependency that should never jump minor versions:

```nix
python3Packages.cryptography = pkgs.python3Packages.cryptography.override {
  passthru.ekapkgs-update = {
    enable = true;
    strategy = "patch";
  };
};
```

Result:
- Current: 41.0.7
- Available: 41.0.8, 41.1.0, 42.0.0
- **Will propose**: 41.0.8
- **Will NOT propose**: 41.1.0, 42.0.0

### Example 2: Minor Updates

Most packages should use this:

```nix
python3Packages.requests = pkgs.python3Packages.requests.override {
  passthru.ekapkgs-update = {
    enable = true;
    strategy = "minor";  # Default, can be omitted
  };
};
```

Result:
- Current: 2.31.0
- Available: 2.31.1, 2.32.0, 3.0.0
- **Will propose**: 2.32.0
- **Will NOT propose**: 3.0.0

### Example 3: Multi-Variant Package

A package with multiple version branches:

```nix
python3Packages.django = mkManyVariants {
  versions = {
    v3_2 = { ... };      # Auto-detected: patch (3.2.z)
    v4_0 = { ... };      # Auto-detected: patch (4.0.z)
    v4_1 = { ... };      # Auto-detected: patch (4.1.z)
    v4_2 = { ... };      # Auto-detected: patch (4.2.z)
    latest = { ... };    # No auto-update (manual only)
  };
};
```

Result for v4_1 variant (current 4.1.12):
- **Will propose**: 4.1.13, 4.1.14, etc.
- **Will NOT propose**: 4.2.0, 5.0.0

### Example 4: Always Latest

Bleeding-edge package:

```nix
unstable-package = {
  passthru.ekapkgs-update = {
    enable = true;
    strategy = "latest";
  };
};
```

Result:
- **Will always propose** the absolute newest version
- No restrictions on major version jumps

## Handling Version Mismatches

Sometimes upstream versions don't match the package's expected format.

### Scenario 1: Version in Metadata

Upstream has version 2.0, but Nix package tracks a different naming:

```nix
pname = "my-package";
version = "2024.05.15";  # Date-based

passthru.ekapkgs-update = {
  enable = true;
  upstream-version-attr = "release-2024-05";  # Custom lookup
};
```

### Scenario 2: Manual Override

If automatic detection fails:

```nix
passthru.ekapkgs-update = {
  enable = true;
  force-version = "1.2.3";  # Use this version, ignore detection
};
```

## Dry-Run Version Selection

Preview what versions would be selected without applying updates:

```bash
ekapkgs-update query python3Packages.requests --strategy patch
# Output:
# Current: 2.31.0
# Strategy: patch
# Latest in range: 2.31.1
# All compatible: 2.31.1, 2.31.0
```

## Performance Considerations

Version matching is performed:
1. **During `run` command**: For all packages being checked
2. **Cached in database**: Results stored in `updates.proposed_version`
3. **Re-evaluated** when `should_check_update()` returns true

Large package sets (1000+) typically take 30-60 seconds to query all upstream version information.

## Troubleshooting

### Version not being selected

Check the strategy and version format:

```bash
# Query why a version wasn't selected
ekapkgs-update query python3Packages.mypkg --strategy minor
# Output may show: "3.0.0 incompatible with strategy=minor, current=2.31.0"
```

### Pre-release versions being preferred

If pre-release is being selected when stable is available, check configuration:

```bash
# Disable pre-release acceptance
ekapkgs-update run --config config.toml --no-prerelease
```

### Non-semver versions failing to match

Some packages use custom versioning. If matching fails:

```nix
passthru.ekapkgs-update = {
  enable = true;
  allow-custom-versions = true;  # Allow non-semver matching
};
```

## Related Topics

- [Configuration](../configuration.md) - Full configuration schema
- [PR Enhancements](./pr-enhancements.md) - Version change type affects PR body
- [Backoff Strategy](./backoff.md) - Version selection affects re-check timing
