# Ekapkgs Update

This is meant to be the spiritual successor to [nixpkgs-update](https://github.com/nix-community/nixpkgs-update)
for Ekapkgs. It will eventually cover the feature set of `nix-update` and `nixpkgs-update` and more.

## Contributing

To build:
```bash
$ nix develop
$ cargo build
```

### Example usage

```bash
$ /home/jon/projects/ekapkgs-update/target/debug/ekapkgs-update update spdlog --ignore-update-script
2025-12-17T01:52:05.168426Z  INFO ekapkgs_update::commands::update: Using semver strategy: Latest
...
2025-12-17T01:52:30.203863Z  INFO ekapkgs_update::commands::update: ✓ Successfully updated spdlog from 1.15.2 to 1.16.0

$ git diff
diff --git a/pkgs/by-name/sp/spdlog/package.nix b/pkgs/by-name/sp/spdlog/package.nix
index 37e08a8dc5a2..e7bce67e0c79 100644
--- a/pkgs/by-name/sp/spdlog/package.nix
+++ b/pkgs/by-name/sp/spdlog/package.nix
@@ -15,13 +15,13 @@

 stdenv.mkDerivation (finalAttrs: {
   pname = "spdlog";
-  version = "1.15.2";
+  version = "1.16.0";

   src = fetchFromGitHub {
     owner = "gabime";
     repo = "spdlog";
     tag = "v${finalAttrs.version}";
-    hash = "sha256-9RhB4GdFjZbCIfMOWWriLAUf9DE/i/+FTXczr0pD0Vg=";
+    hash = "sha256-VB82cNfpJlamUjrQFYElcy0CXAbkPqZkD5zhuLeHLzs=";
   };

   nativeBuildInputs = [ cmake ];
```

### CVE/Vulnerability Checking

ekapkgs-update automatically checks for known security vulnerabilities when running in daemon mode (`run` command). This feature uses [OSV.dev](https://osv.dev) to query vulnerability data across multiple ecosystems.

**Features:**
- Automatically detects package ecosystems (PyPI, crates.io, npm, Packagist, NuGet)
- Shows CVEs resolved, introduced, or present in both versions
- Includes severity levels and links to vulnerability details
- 24-hour caching to reduce API calls
- No rate limits (OSV.dev is free and open source)

**Example PR output:**
```markdown
## Security

### CVEs Resolved ✅
- [CVE-2024-1234](https://osv.dev/CVE-2024-1234) - Critical: Remote code execution
- [GHSA-xxxx-yyyy-zzzz](https://osv.dev/GHSA-xxxx-yyyy-zzzz) - High: SQL injection

### CVEs Present in Both Versions
- [CVE-2023-5678](https://osv.dev/CVE-2023-5678) - Low: Information disclosure (not patched)
```

**Disabling CVE checks:**
```bash
$ ekapkgs-update run --no-cve
```

### Repology Integration

ekapkgs-update integrates with [Repology.org](https://repology.org) to validate versions across multiple Linux distributions. This provides additional confidence that the detected upstream version is stable and adopted by other distributions.

**Features:**
- Cross-distribution version validation
- Automatic package name normalization (python3-foo → foo, etc.)
- 72-hour caching to respect Repology API rate limits (1 req/sec)
- Fallback version discovery when upstream checks fail
- Works seamlessly with PyPI, crates.io, npm, and other ecosystems

**How it works:**
1. After finding the latest upstream version, checks Repology to see if other distributions agree
2. Logs informational messages when Repology reports a different "newest" version
3. Uses Repology as a fallback when upstream API calls fail

**Example log output:**
```
INFO firefox: Latest version: 125.0.1
DEBUG firefox: Repology confirms 125.0.1 is newest across distributions
```

**Disabling Repology checks:**
```bash
$ ekapkgs-update run --no-repology
```

# Roadmap

Update feature set
- [x] nix-update-script support
  - This is now the default behavior, use '--ignore-update-script' if it attempts to run it
- [x] mkManyVariant support
- [x] Version rewriting
- [x] Test updated expression
- [x] Retain failed updates
- [x] Remove already applied patches (currently only supports pruning one patch)

Daemon and web features
- [x] CVE/Vulnerability Integration
  - Automatically checks for security vulnerabilities using OSV.dev
  - Displays CVEs fixed, introduced, or present in PR descriptions
  - 24-hour caching to minimize API calls
  - Use `--no-cve` flag to disable if needed
- [x] Repology Integration
  - Cross-distribution version validation via Repology.org
  - Helps confirm version numbers are stable and widely adopted
  - 72-hour caching with 1 req/sec rate limiting
  - Use `--no-repology` flag to disable if needed
- [ ] Batch evaluation
- [ ] Website for exploring failing updates

# Future features

- [ ]: Automatic fixing of trivial build issues
  - e.g. Missing dependency which is available
