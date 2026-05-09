# PR Enhancement Features

> **Note:** This chapter is under construction.

ekapkgs-update can automatically enhance pull requests with:

## Features

- **CVE Checking** - Automatic vulnerability scanning
- **Repology Integration** - Cross-distribution version verification
- **Directory Diff** - Show package size changes
- **Rebuild Analysis** - Count affected packages

## Usage

```bash
# With all enhancements
ekapkgs-update update mypackage --create-pr

# Skip specific enhancements
ekapkgs-update run --skip-cve --skip-repology
```
