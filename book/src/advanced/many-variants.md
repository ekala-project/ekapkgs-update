# mkManyVariants Packages

> **Note:** This chapter is under construction.

Special handling for packages that use `mkManyVariants`.

## Overview

mkManyVariants packages can have per-variant passthru attributes:

```nix
mkManyVariants {
  variants = {
    v1_0 = {
      passthru.ekapkgs-update.semver-strategy = "patch";
    };
    v2_0 = {
      passthru.ekapkgs-update.semver-strategy = "minor";
    };
  };
}
```

## Strategy Inference

Strategies are inferred from variant names if not specified:

- `v1` → `minor` (1.x.x)
- `v1_2` → `patch` (1.2.x)
- `v1_2_3` → pinned (no updates)
