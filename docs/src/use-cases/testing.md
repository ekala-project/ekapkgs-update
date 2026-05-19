# Testing Update Workflows

Strategies for testing package updates before deploying to production.

## Test Levels

### 1. Dry-Run Testing

```bash
# Preview updates without modifications
ekapkgs-update run --dry-run

# Check specific package
ekapkgs-update update mypackage --dry-run
```

**What it checks:**
- Available updates
- Version compatibility
- API rate limits
- VCS source connectivity

**What it skips:**
- File modifications
- Hash computation
- Building
- Committing/PRs

### 2. Build Testing

```bash
# Update and build, but don't commit
ekapkgs-update update mypackage

# Verify build
nix-build -A mypackage

# Test manually
./result/bin/mypackage --version
./result/bin/mypackage --help
```

### 3. Automated Testing

```bash
# Run package's passthru.tests
ekapkgs-update update mypackage --run-passthru-tests

# Batch with tests
ekapkgs-update run --run-passthru-tests
```

### 4. Integration Testing

```bash
# Update in test environment first
ekapkgs-update update mypackage --commit

# Build dependent packages
nix-build -A package-that-depends-on-mypackage

# Run integration tests
nix-build -A mypackage.tests.integration
```

## Test Infrastructure

### Basic Version Test

```nix
{
  mypackage = pkgs.stdenv.mkDerivation {
    # ... package definition ...

    passthru.tests = {
      version = pkgs.runCommand "${pname}-test-version" {} ''
        ${finalAttrs.finalPackage}/bin/${pname} --version | grep "${version}"
        touch $out
      '';
    };
  };
}
```

### Comprehensive Test Suite

```nix
{
  mypackage = pkgs.stdenv.mkDerivation rec {
    pname = "mypackage";
    version = "1.0.0";

    # ... package definition ...

    passthru.tests = {
      # Version check
      version = pkgs.runCommand "${pname}-test-version" {} ''
        ${finalAttrs.finalPackage}/bin/${pname} --version | grep "${version}"
        touch $out
      '';

      # Help text
      help = pkgs.runCommand "${pname}-test-help" {} ''
        ${finalAttrs.finalPackage}/bin/${pname} --help
        touch $out
      '';

      # Basic functionality
      basic = pkgs.runCommand "${pname}-test-basic" {} ''
        echo "test input" | ${finalAttrs.finalPackage}/bin/${pname} > output.txt
        grep "expected" output.txt
        touch $out
      '';

      # Python imports (for Python packages)
      python-imports = pkgs.runCommand "${pname}-test-imports" {
        nativeBuildInputs = [ pkgs.python312 ];
      } ''
        python -c "import ${pname}; print(${pname}.__version__)"
        touch $out
      '';
    };
  };
}
```

### NixOS Tests

```nix
{
  mypackage = pkgs.stdenv.mkDerivation rec {
    # ... package definition ...

    passthru.tests = {
      # Unit tests
      version = ...;

      # Integration test with NixOS
      nixos = pkgs.nixosTest {
        name = "${pname}-nixos-test";

        nodes.machine = { pkgs, ... }: {
          environment.systemPackages = [ pkgs.mypackage ];
          services.mypackage = {
            enable = true;
            port = 8080;
          };
        };

        testScript = ''
          machine.start()
          machine.wait_for_unit("mypackage.service")
          machine.wait_for_open_port(8080)
          machine.succeed("curl http://localhost:8080/health")
          machine.succeed("mypackage --version")
        '';
      };
    };
  };
}
```

## Testing Strategies

### Progressive Testing

```bash
# 1. Dry-run first
ekapkgs-update run --dry-run

# 2. Update low-impact packages
ekapkgs-update run --max-rebuilds 10

# 3. Test critical packages manually
ekapkgs-update update gcc --run-passthru-tests
ekapkgs-update update python3 --run-passthru-tests

# 4. Full batch update
ekapkgs-update run --run-passthru-tests
```

### Canary Testing

```bash
# Update in test branch first
git checkout -b test-updates

ekapkgs-update run --skip-unstable --max-rebuilds 50

# Test builds
nix-build -A criticalPackage1
nix-build -A criticalPackage2

# Integration tests
nix-build -A integration-tests

# Merge if successful
git checkout main
git merge test-updates
```

### Regression Testing

```bash
# Before update: record current state
nix-build -A mypackage
cp result/bin/mypackage mypackage-old

# Update
ekapkgs-update update mypackage

# After update: compare
nix-build -A mypackage
./result/bin/mypackage --version
./mypackage-old --version

# Run diff tests
diff <(./mypackage-old --help) <(./result/bin/mypackage --help)
```

## CI/CD Testing

### Pre-merge Testing

```yaml
# .github/workflows/test-pr.yml
name: Test Package Updates

on:
  pull_request:
    paths:
      - 'pkgs/**/*.nix'

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: DeterminateSystems/nix-installer-action@main

      - name: Build changed packages
        run: |
          # Detect changed packages
          git diff origin/main --name-only | \
            grep '\.nix$' | \
            xargs -I {} nix-build -A {}

      - name: Run package tests
        run: |
          # Run tests for changed packages
          nix-build -A changedPackage.tests
```

### Scheduled Testing

```yaml
# .github/workflows/nightly-tests.yml
name: Nightly Package Tests

on:
  schedule:
    - cron: '0 0 * * *'

jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        package:
          - python312Packages.requests
          - nodejs
          - terraform
          - gcc

    steps:
      - uses: actions/checkout@v4

      - uses: DeterminateSystems/nix-installer-action@main

      - name: Test ${{ matrix.package }}
        run: |
          nix-build -A ${{ matrix.package }}
          nix-build -A ${{ matrix.package }}.tests
```

## Failure Testing

### Intentional Failure Testing

```bash
# Test failure preservation
ekapkgs-update update mypackage --preserve-failures

# Verify worktree created
ekapkgs-update worktrees show mypackage

# Test retry workflow
ekapkgs-update retry mypackage
```

### Error Recovery Testing

```bash
# Test with known-bad version
ekapkgs-update update mypackage --version 99.99.99 --preserve-failures
# Should fail

# Test export
ekapkgs-update export mypackage --format markdown

# Test retry with fix
ekapkgs-update retry mypackage --version 2.5.0
# Should succeed
```

## Performance Testing

### Rebuild Analysis

```bash
# Test impact of update
ekapkgs-update update mypackage --analyze-rebuilds

# Check rebuild count
# If too high, consider:
# - Splitting into smaller updates
# - Updating during low-traffic periods
# - Using Cachix to pre-build
```

### Timing Tests

```bash
# Benchmark update performance
time ekapkgs-update update mypackage

# Benchmark with different concurrency
time ekapkgs-update run --concurrent-updates 1
time ekapkgs-update run --concurrent-updates 4
time ekapkgs-update run --concurrent-updates 8
```

## Test Environments

### Isolated Test Environment

```bash
# Create isolated environment
nix-shell -p ekapkgs-update

# Test in isolation
mkdir /tmp/test-updates
cd /tmp/test-updates
git clone https://github.com/example/repo.git
cd repo

ekapkgs-update run --dry-run --database /tmp/test-db.sqlite3
```

### Docker Test Environment

```dockerfile
# Dockerfile.test
FROM nixos/nix:latest

COPY . /workspace
WORKDIR /workspace

RUN nix profile install .#ekapkgs-update

CMD ["ekapkgs-update", "run", "--dry-run"]
```

```bash
# Run tests in Docker
docker build -f Dockerfile.test -t test-updates .
docker run test-updates
```

## Validation Checklist

After updating, verify:

### Package Level
- [ ] Package builds successfully
- [ ] `--version` output is correct
- [ ] `--help` works
- [ ] Basic functionality works
- [ ] passthru.tests pass
- [ ] No new warnings in build log

### System Level
- [ ] Dependent packages still build
- [ ] No circular dependencies introduced
- [ ] Closure size hasn't increased dramatically
- [ ] Update doesn't break NixOS modules

### Repository Level
- [ ] All affected packages build
- [ ] CI pipeline passes
- [ ] No merge conflicts
- [ ] Documentation updated if needed

## Automated Validation

```bash
#!/bin/bash
# validate-update.sh

PACKAGE="$1"

echo "Validating $PACKAGE update..."

# Build package
echo "1. Building package..."
if ! nix-build -A "$PACKAGE"; then
    echo "✗ Build failed"
    exit 1
fi
echo "✓ Build succeeded"

# Run tests
echo "2. Running tests..."
if ! nix-build -A "$PACKAGE.tests" 2>/dev/null; then
    echo "⚠ No tests or tests failed"
else
    echo "✓ Tests passed"
fi

# Check dependents
echo "3. Checking dependents..."
DEPENDENTS=$(nix-instantiate --eval --expr "
  with import <nixpkgs> {};
  lib.attrNames (lib.filterAttrs (n: v:
    builtins.elem \"$PACKAGE\" (v.buildInputs or [])
  ) pkgs)
" | jq -r '.[]')

for dep in $DEPENDENTS; do
    echo "  Testing dependent: $dep"
    if ! nix-build -A "$dep" >/dev/null 2>&1; then
        echo "  ✗ $dep failed"
    fi
done

echo "✓ Validation complete"
```

## See Also

- [update command](../cli/update.md) - Update options
- [Batch Updates](./batch-updates.md) - Testing batch updates
- [Debugging](./debugging.md) - Troubleshooting failures
- [CI/CD Integration](./ci-cd.md) - Automated testing
