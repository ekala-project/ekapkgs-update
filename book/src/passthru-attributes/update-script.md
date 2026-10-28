# Update Script

The `update-script` attribute provides a custom script that lists available upstream versions. Unlike [`passthru.updateScript`](../../docs/src/reference/package-schema.md) (which takes over the entire update process), `update-script` only handles version discovery — ekapkgs-update manages everything else (semver filtering, hash updates, patch recovery, commit creation).

## Syntax

```nix
passthru.ekapkgs-update.update-script = writeScript "list-releases" ''
  # Print one version per line to stdout
  ${curl}/bin/curl -s https://example.com/releases/ \
    | ${pcregrep}/bin/pcregrep -o1 'pkg-([0-9]+\.[0-9]+\.[0-9]+)\.tar' \
    | sort -u
'';
```

**Type:** `derivation` (script that prints versions to stdout, one per line)
**Default:** `null` (use auto-detection from `src.url`)
**Implemented:** Yes

## When to Use

Use `update-script` when the package's upstream releases cannot be auto-detected from `src.url` and there is no GitHub/GitLab/PyPI/SourceHut repository to point `github-repo` at.

Common cases:
- Packages fetched from custom download pages (cmake.org, lua.org, openssl.org)
- Packages using `builtins.fetchurl` with no parseable URL
- Packages where the release listing requires scraping or an unusual API

If the package is hosted on GitHub, GitLab, SourceHut, or PyPI, you **don't need** `update-script` — the tool auto-detects the upstream source from `src.url`. If the URL isn't parseable but the project has a GitHub repo, use `github-repo` instead (it's simpler).

## Priority

`update-script` has the **highest priority** in upstream source resolution:

1. `update-script` — custom version-listing script
2. `github-repo` — explicit `"owner/repo"` override
3. `src.url` parsing — auto-detect from GitHub, GitLab, SourceHut, PyPI, GNU, SourceForge
4. `pname` — assume PyPI package as last resort

## Script Requirements

The script must:
- Print available version strings to **stdout**, one per line
- Exit with status 0 on success
- **Not modify** any files — it only lists versions

The script must **not**:
- Rewrite Nix files
- Update hashes
- Create commits

ekapkgs-update handles all of that after the script runs.

## Examples

### CMake (scraping a download page)

```nix
{
  pname = "cmake";
  version = "3.30.2";

  src = fetchurl {
    url = "https://cmake.org/files/v${lib.versions.majorMinor version}/cmake-${version}.tar.gz";
    hash = "sha256-...";
  };

  passthru.ekapkgs-update = {
    update-script = writeScript "list-cmake-releases" ''
      ${curl}/bin/curl -s https://cmake.org/files/ \
        | ${pcregrep}/bin/pcregrep -o1 'v([0-9]+\.[0-9]+\.[0-9]+)/' \
        | sort -uV
    '';
    semver-strategy = "patch";
  };
}
```

### OpenSSL (tarball directory listing)

```nix
passthru.ekapkgs-update = {
  update-script = writeScript "list-openssl-releases" ''
    ${curl}/bin/curl -s https://www.openssl.org/source/ \
      | ${pcregrep}/bin/pcregrep -o1 'openssl-([0-9]+\.[0-9]+\.[0-9]+)\.tar' \
      | sort -uV
  '';
  semver-strategy = "patch";
};
```

### Using a JSON API

```nix
passthru.ekapkgs-update = {
  update-script = writeScript "list-releases" ''
    ${curl}/bin/curl -s https://api.example.com/releases \
      | ${jq}/bin/jq -r '.[].version'
  '';
};
```

## Interaction with Other Attributes

### Combined with semver-strategy

The script lists all versions; `semver-strategy` filters them:

```nix
passthru.ekapkgs-update = {
  update-script = ./list-versions.sh;
  semver-strategy = "patch";
};

# Script outputs: 3.28.1, 3.29.0, 3.29.1, 3.30.0
# If current is 3.29.0, semver-strategy = "patch" selects: 3.29.1
```

### Combined with version-regex

If the script outputs tag-like strings rather than clean versions, `version-regex` can extract the version:

```nix
passthru.ekapkgs-update = {
  update-script = ./list-tags.sh;
  version-regex = "release-(.*)";
};

# Script outputs: release-1.2.3, release-1.3.0
# Regex extracts: 1.2.3, 1.3.0
```

### Combined with include-prereleases

Prerelease detection works on the version strings the script outputs:

```nix
passthru.ekapkgs-update = {
  update-script = ./list-versions.sh;
  include-prereleases = true;
};

# Script outputs: 1.2.3, 1.3.0-rc1, 1.3.0
# With include-prereleases = true: all three are candidates
# With include-prereleases = false (default): 1.3.0-rc1 is skipped
```

## Comparison with passthru.updateScript

| | `ekapkgs-update.update-script` | `passthru.updateScript` |
|---|---|---|
| **Scope** | Lists versions only | Does everything (fetch, rewrite, hash) |
| **Hash updates** | Handled by ekapkgs-update | Script must do it |
| **Patch recovery** | Automatic | Not available |
| **Commit/PR creation** | Automatic | Script must do it |
| **Semver filtering** | Works | Not available |
| **keep mode** | Works | Not available |
| **Recommended** | Yes | Legacy fallback |

## Related

- [Semver Strategy](./semver-strategy.md) — applied after version listing
- [Version Regex](./version-regex.md) — extract version from script output
- [Include Prereleases](./include-prereleases.md) — filter prerelease versions
- [Keep](./keep.md) — version retention for mkManyVariants
