# report - Failure Reports

The `report` command generates a categorized markdown summary of all failed updates from the database.

## Synopsis

```bash
ekapkgs-update report [OPTIONS]
```

## Description

After a batch update run, use `report` to get a structured overview of what failed and why. Failures are automatically categorized by error type (build failures, hash discovery issues, evaluation errors, etc.) and rendered as a markdown document with tables.

The report deduplicates entries by package — if a package failed multiple times, only the most recent failure is shown.

## Options

### `--database <PATH>` (short: `-d`)
Path to SQLite database.

**Default:** `~/.cache/ekapkgs-update/updates.db`

### `--package <PATTERN>`
Filter by package name using SQL LIKE pattern.

```bash
# Only Python 3.12 packages
ekapkgs-update report --package "python312Packages.%"

# Only AWS packages
ekapkgs-update report --package "aws-%"
```

### `--since-days <N>`
Filter to failures from the last N days.

```bash
# Failures from the last 24 hours
ekapkgs-update report --since-days 1
```

### `--output <PATH>` (short: `-o`)
Write the report to a file instead of stdout.

```bash
ekapkgs-update report -o failed-updates.md
```

## Failure Categories

Failures are automatically categorized by matching patterns in the error log:

| Category | Meaning |
|----------|---------|
| **Build failures** | Package compiled but the build failed |
| **Cargo hash discovery failures** | Could not determine the correct `cargoHash` for Rust packages |
| **Vendor hash discovery failures** | Could not determine the correct `vendorHash` for Go packages |
| **Hash discovery failures** | Could not extract the correct source hash |
| **Source build failures** | Source fetching or extraction failed |
| **Patch removal failures** | An obsolete patch was detected but could not be automatically removed |
| **Attribute not found (eval issues)** | Nix evaluation failed — typically mkManyVariants packages where the version attribute is structured differently |
| **No compatible release found** | The upstream version detected by the checker could not be resolved by the updater |
| **Other failures** | Anything not matching the above patterns |

## Example Output

```markdown
# Failed Updates

Total: 265 packages failed

## Build failures (87)

| Package | Old | New |
|---------|-----|-----|
| `aws-c-auth` | 0.9.1 | 0.10.3 |
| `brotli` | 1.1.0 | 1.2.0 |
| ...

## Cargo hash discovery failures (8)

| Package | Old | New |
|---------|-----|-----|
| `aardvark-dns` | 1.17.1 | 2.0.0 |
| ...
```

## Examples

```bash
# Full report to stdout
ekapkgs-update report

# Save to file
ekapkgs-update report -o failed-updates.md

# Only Python packages, last week
ekapkgs-update report --package "python%" --since-days 7

# Only Rust-related failures
ekapkgs-update report --package "%rust%"
```

## See Also

- [query](./query.md) — interactive failure search with table output
- [log / inspect](./inspect.md) — detailed failure information for a specific package
- [retry](./retry.md) — retry a failed update
