# Version Regex

The `version-regex` attribute provides a custom regular expression for extracting version numbers from Git tags.

## Syntax

```nix
passthru.ekapkgs-update.version-regex = "pattern-with-(capture)";
```

**Type:** `string` (regex with one capture group)
**Default:** `null` (use default tag parsing)
**Implemented:** ✅ Yes

## When to Use

Use `version-regex` when upstream uses non-standard tag formats:

### Tags with Prefixes

```nix
passthru.ekapkgs-update.version-regex = "jq-(.*)";
# Extracts "1.6" from tag "jq-1.6"
```

### Tags with Project Names

```nix
passthru.ekapkgs-update.version-regex = "myapp-v(.*)";
# Extracts "2.3.1" from tag "myapp-v2.3.1"
```

### Custom Release Naming

```nix
passthru.ekapkgs-update.version-regex = "release-(.*)";
# Extracts "2023.10" from tag "release-2023.10"
```

## Regex Requirements

### Must Have Exactly One Capture Group

```nix
# ✅ Correct: One capture group
version-regex = "v(.*)";
version-regex = "release-([0-9.]+)";
version-regex = "prefix-(.*)-suffix";

# ❌ Wrong: No capture group
version-regex = "v.*";

# ❌ Wrong: Multiple capture groups
version-regex = "(v)(.*)";
```

### Capture Group Contains the Version

The captured text becomes the version string:

```nix
version-regex = "jq-(.*)";
# Tag: jq-1.6
# Captured: "1.6"
# Used as: version = "1.6"
```

## Default Behavior

Without `version-regex`, the tool strips common prefixes:

```nix
# No version-regex specified
passthru.ekapkgs-update = { };

# Default extraction:
# v1.2.3      → 1.2.3
# V1.2.3      → 1.2.3
# version1.2.3 → 1.2.3
# 1.2.3       → 1.2.3
```

This works for most projects. Use `version-regex` only when needed.

## Examples

### jq-style Tags

```nix
{
  pname = "jq";
  version = "1.6";

  src = fetchFromGitHub {
    owner = "stedolan";
    repo = "jq";
    rev = "jq-${version}";  # Tag format: jq-1.6
    # ...
  };

  passthru.ekapkgs-update.version-regex = "jq-(.*)";
}
```

### Release Prefix

```nix
{
  pname = "myapp";
  version = "2.5.1";

  src = fetchFromGitHub {
    # Tags: release-2.5.1, release-2.5.0
    # ...
  };

  passthru.ekapkgs-update.version-regex = "release-(.*)";
}
```

### Semantic Versions Only

Filter out non-semantic versions:

```nix
{
  pname = "mixed-tags";

  # Tags: v1.2.3, v1.3.0, nightly-2023-10-15, docs-update
  passthru.ekapkgs-update.version-regex = "v([0-9]+\\.[0-9]+\\.[0-9]+)";
  # Only matches: v1.2.3, v1.3.0
}
```

### Version Range Filtering

Only accept versions in specific range:

```nix
{
  pname = "legacy-app";

  # Tags: v1.5.0, v2.1.0, v2.2.0, v3.0.0
  passthru.ekapkgs-update = {
    version-regex = "v(2\\.[0-9]+\\.[0-9]+)";  # Only v2.x.x
  };
  # Matches: v2.1.0, v2.2.0
  # Ignores: v1.5.0, v3.0.0
}
```

### Complex Tag Format

```nix
{
  pname = "complex";

  # Tags: myapp-v1.2.3-stable, myapp-v1.3.0-stable
  passthru.ekapkgs-update.version-regex = "myapp-v(.*)-stable";
}
```

## Interaction with Other Attributes

### Combined with semver-strategy

Regex extraction happens **before** semver filtering:

```nix
passthru.ekapkgs-update = {
  version-regex = "release-(.*)";  # Extract version
  semver-strategy = "minor";        # Then filter by semver
};

# Tags: release-1.2.3, release-1.3.0, release-2.0.0
# Extracted: 1.2.3, 1.3.0, 2.0.0
# If current is 1.2.3, semver filters to: 1.2.3, 1.3.0
# Winner: 1.3.0
```

### Combined with include-prereleases

Prerelease detection works on the **extracted** version:

```nix
passthru.ekapkgs-update = {
  version-regex = "v(.*)";
  include-prereleases = true;
};

# Tag: v1.3.0-beta.1
# Extracted: 1.3.0-beta.1
# Prerelease detected from extracted version (has "-beta")
```

## Precedence

For manual updates, passthru version-regex **takes precedence** over CLI:

```nix
# In package definition
passthru.ekapkgs-update.version-regex = "jq-(.*)";
```

```bash
# This CLI argument is IGNORED if passthru is set
ekapkgs-update update jq --version-regex "release-(.*)"
```

To override, you must remove the passthru attribute.

## Testing Your Regex

### Test Locally

```bash
# Test extraction with grep
echo "jq-1.6" | grep -oP 'jq-(.*)'
# Output: 1.6

# Test with multiple tags
printf "jq-1.5\njq-1.6\nother-tag\n" | grep -oP 'jq-(.*)'
# Output:
# 1.5
# 1.6
```

### Test in Nix

```bash
# See what versions would be extracted
RUST_LOG=debug ekapkgs-update update myapp 2>&1 | grep -i "version extraction"
```

### Common Regex Patterns

```nix
# Basic prefix
"v(.*)"                           # v1.2.3 → 1.2.3

# Project name prefix
"projectname-(.*)"                # projectname-1.2.3 → 1.2.3

# Prefix and suffix
"release-(.*)-final"              # release-1.2.3-final → 1.2.3

# Semver only
"v([0-9]+\\.[0-9]+\\.[0-9]+)"    # v1.2.3 → 1.2.3 (ignores v1.2.3-beta)

# Specific version range
"v(2\\.[0-9]+\\.[0-9]+)"         # Only v2.x.x versions

# CalVer format
"v([0-9]{4}\\.[0-9]{2})"         # v2023.10 → 2023.10
```

## Troubleshooting

### Regex Not Working

If version extraction fails:

1. **Check capture group count**:
   ```nix
   # ❌ No capture group
   version-regex = "v.*";

   # ✅ One capture group
   version-regex = "v(.*)";
   ```

2. **Test the regex**:
   ```bash
   echo "your-tag" | grep -oP 'your-regex'
   ```

3. **Check escaping**:
   ```nix
   # In Nix strings, backslashes need doubling
   "v([0-9]+\\.[0-9]+)"  # ✅ Correct
   "v([0-9]+\.[0-9]+)"   # ❌ Wrong (single backslash)
   ```

4. **Enable debug logging**:
   ```bash
   RUST_LOG=debug ekapkgs-update update myapp 2>&1 | grep regex
   ```

### No Versions Found

If no versions match your regex:

1. **Check actual tag format**:
   ```bash
   gh api repos/owner/repo/tags | jq '.[].name'
   ```

2. **Try a more permissive regex**:
   ```nix
   # Start broad
   version-regex = "(.*)";  # Matches everything

   # Then narrow down
   version-regex = "release-(.*)";
   ```

3. **Verify the regex is being used**:
   ```bash
   nix-instantiate --eval -E 'with import ./default.nix {}; myapp.passthru.ekapkgs-update.version-regex'
   ```

### Wrong Version Selected

If an unexpected version is selected:

1. **Check what's being extracted**:
   ```bash
   RUST_LOG=debug ekapkgs-update update myapp 2>&1 | grep -i "extracted\|resolved"
   ```

2. **Test extraction manually**:
   ```bash
   echo "tag-name" | grep -oP 'your-regex'
   ```

3. **Refine the regex**:
   ```nix
   # Too broad
   version-regex = "(.*)";

   # More specific
   version-regex = "v([0-9]+\\.[0-9]+\\.[0-9]+)";
   ```

## Best Practices

### 1. Keep It Simple

Use the simplest regex that works:

```nix
# Simple prefix removal
version-regex = "jq-(.*)";

# Don't overcomplicate
version-regex = "jq-([vV]?[0-9]+\\.[0-9]+(?:\\.[0-9]+)?)";  # Overkill
```

### 2. Document the Tag Format

```nix
passthru.ekapkgs-update = {
  # Upstream uses "jq-X.Y" tag format
  version-regex = "jq-(.*)";
};
```

### 3. Combine with Semver Strategy

```nix
passthru.ekapkgs-update = {
  version-regex = "release-(.*)";  # Extract version
  semver-strategy = "minor";        # Then apply constraints
};
```

### 4. Test Before Committing

```bash
# Dry run to verify regex works
ekapkgs-update update myapp --dry-run
```

## Common Patterns

### Standard Prefixes

```nix
# v prefix
version-regex = "v(.*)";

# V prefix (case-sensitive)
version-regex = "V(.*)";

# version prefix
version-regex = "version(.*)";

# release prefix
version-regex = "release-(.*)";
```

### Project Name in Tags

```nix
# project-1.2.3
version-regex = "project-(.*)";

# project-v1.2.3
version-regex = "project-v(.*)";

# project/1.2.3
version-regex = "project/(.*)";
```

### Filtering Specific Branches

```nix
# Only v2.x tags
version-regex = "v(2\\.[0-9]+\\.[0-9]+)";

# Only stable tags (exclude beta/rc)
version-regex = "v([0-9]+\\.[0-9]+\\.[0-9]+)$";
```

## Examples by Ecosystem

### Rust/Cargo Projects

```nix
version-regex = "v(.*)";  # Usually v-prefixed
```

### Go Projects

```nix
version-regex = "v(.*)";  # Standard v prefix
```

### Python Projects

```nix
# No regex needed usually, but sometimes:
version-regex = "release-(.*)";
```

### JavaScript/Node

```nix
version-regex = "v(.*)";  # npm publish creates v-prefixed tags
```

## Related

- [Semver Strategy](./semver-strategy.md) - Applied after regex extraction
- [Include Prereleases](./include-prereleases.md) - Detected from extracted version
- [CLI Reference](../cli-reference.md) - Manual `--version-regex` flag
