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
- [ ] Batch evaluation
- [ ] Website for exploring failing updates

# Future features

- [ ]: Automatic fixing of trivial build issues
  - e.g. Missing dependency which is available
