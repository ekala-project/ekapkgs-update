# Ekapkgs Update

This is meant to be the spiritual successor to [nixpkgs-update](https://github.com/nix-community/nixpkgs-update)
for Ekapkgs. It will eventually cover the feature set of `nix-update` and `nixpkgs-update` and more.

---

## 📚 Documentation

**📖 Read the full documentation at: https://ekala-project.github.io/ekapkgs-update/**

### Quick Links

- **[Installation Guide](https://ekala-project.github.io/ekapkgs-update/installation.html)** - Get started with ekapkgs-update
- **[Quick Start](https://ekala-project.github.io/ekapkgs-update/quick-start.html)** - Common workflows and examples
- **[CLI Reference](https://ekala-project.github.io/ekapkgs-update/cli-reference.html)** - Complete command documentation
- **[Passthru Attributes (EEP-0039)](https://ekala-project.github.io/ekapkgs-update/passthru-attributes.html)** - Per-package configuration
- **[Configuration](https://ekala-project.github.io/ekapkgs-update/configuration.html)** - Environment variables and settings
- **[Usage Guide](https://ekala-project.github.io/ekapkgs-update/usage.html)** - Manual updates and daemon mode
- **[Contributing Guide](https://ekala-project.github.io/ekapkgs-update/contributing/development.html)** - Development setup and guidelines

---

## Quick Start

### Installation

```bash
# Using Nix
nix-shell -p ekapkgs-update

# Or with flakes
nix run github:ekala-project/ekapkgs-update -- --help
```

**[→ Full installation instructions](https://ekala-project.github.io/ekapkgs-update/installation.html)**

### Basic Usage

```bash
# Update a single package
ekapkgs-update update mypackage

# Update with commit
ekapkgs-update update mypackage --commit

# Update and create PR
ekapkgs-update update mypackage --create-pr

# Run daemon mode (continuous updates)
ekapkgs-update run --file ./default.nix
```

**[→ See all usage examples in the Quick Start guide](https://ekala-project.github.io/ekapkgs-update/quick-start.html)**

### Example Output

```bash
$ ekapkgs-update update spdlog
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

---

## Features

### 🔧 Per-Package Configuration (EEP-0039)

Configure update behavior directly in package definitions:

```nix
passthru.ekapkgs-update = {
  skip = false;                    # Enable/disable updates
  semver-strategy = "minor";       # Version constraints (latest, major, minor, patch)
  include-prereleases = false;     # Prerelease handling
  version-regex = "v(.*)";         # Custom version extraction
};
```

**[→ Read the complete Passthru Attributes guide](https://ekala-project.github.io/ekapkgs-update/passthru-attributes.html)**

### 🔒 CVE/Vulnerability Checking

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
$ ekapkgs-update run --skip-cve
```

**[→ See full PR enhancement documentation](https://ekala-project.github.io/ekapkgs-update/advanced/pr-enhancements.html)**

### 🌍 Repology Integration

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
$ ekapkgs-update run --skip-repology
```

**[→ See full PR enhancement documentation](https://ekala-project.github.io/ekapkgs-update/advanced/pr-enhancements.html)**

---

## Contributing

Want to contribute? Check out the **[Contributing Guide](https://ekala-project.github.io/ekapkgs-update/contributing/development.html)** for:
- Development setup instructions
- Testing guidelines
- Code style requirements
- How to add new features

**[→ Read the full contributing guide](https://ekala-project.github.io/ekapkgs-update/contributing/development.html)**

---

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
  - Use `--skip-cve` flag to disable if needed
- [x] Repology Integration
  - Cross-distribution version validation via Repology.org
  - Helps confirm version numbers are stable and widely adopted
  - 72-hour caching with 1 req/sec rate limiting
  - Use `--skip-repology` flag to disable if needed
- [ ] Batch evaluation
- [ ] Website for exploring failing updates

# Future features

- [ ]: Automatic fixing of trivial build issues
  - e.g. Missing dependency which is available

---

## 📖 Documentation & Resources

- **[Full Documentation](https://ekala-project.github.io/ekapkgs-update/)** - Complete user and developer documentation
- **[Installation Guide](https://ekala-project.github.io/ekapkgs-update/installation.html)** - How to install ekapkgs-update
- **[Quick Start](https://ekala-project.github.io/ekapkgs-update/quick-start.html)** - Get started quickly with examples
- **[CLI Reference](https://ekala-project.github.io/ekapkgs-update/cli-reference.html)** - Complete command documentation
- **[Passthru Attributes](https://ekala-project.github.io/ekapkgs-update/passthru-attributes.html)** - Per-package configuration (EEP-0039)
- **[Configuration](https://ekala-project.github.io/ekapkgs-update/configuration.html)** - Environment variables and settings
- **[Usage Guide](https://ekala-project.github.io/ekapkgs-update/usage.html)** - Manual updates and daemon mode
- **[Troubleshooting](https://ekala-project.github.io/ekapkgs-update/troubleshooting.html)** - Common issues and solutions
- **[Contributing](https://ekala-project.github.io/ekapkgs-update/contributing/development.html)** - How to contribute
- **[Architecture](https://ekala-project.github.io/ekapkgs-update/contributing/architecture.html)** - Code structure and design

## License

[License information here]
