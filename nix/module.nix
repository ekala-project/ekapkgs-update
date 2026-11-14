{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.ekapkgs-update;
  settingsFormat = pkgs.formats.toml { };

  # The Cachix auth token can be supplied two ways:
  #
  # - environmentFile  → a path containing `CACHIX_AUTH_TOKEN=...` (and
  #                      optionally GITHUB_TOKEN, etc.). Loaded by systemd
  #                      via `EnvironmentFile=`. This is the default and
  #                      composes cleanly with sops-nix / agenix.
  #
  # - cachix.authTokenFile → a path containing the bare token value (no
  #                          `KEY=` prefix). Loaded via systemd
  #                          `LoadCredential=` and exported by a helper.
  useCredential = cfg.cachix.authTokenFile != null;

  # Recursively strip null values and empty attrsets so that TOML
  # serialization doesn't fail with "unsupported unit type".
  removeNulls =
    let
      go =
        v:
        if builtins.isAttrs v then
          let
            filtered = lib.filterAttrs (_: val: val != null) (builtins.mapAttrs (_: go) v);
          in
          lib.filterAttrs (_: val: !(builtins.isAttrs val && val == { })) filtered
        else if builtins.isList v then
          map go v
        else
          v;
    in
    go;

  # Merge user settings with computed defaults.
  effectiveSettings = removeNulls (
    lib.recursiveUpdate {
      database = "/var/lib/${cfg.stateDirectory}/updates.db";
      file = cfg.packagesFile;
    } cfg.settings
  );

  configFile = settingsFormat.generate "ekapkgs-update.toml" effectiveSettings;

  # Watch mode effective settings: merge defaults with user-supplied watch settings.
  watchEffectiveSettings = removeNulls (
    lib.recursiveUpdate {
      database = "/var/lib/${cfg.stateDirectory}/updates.db";
      file = cfg.packagesFile;
      repos = map (r: {
        inherit (r)
          name
          upstream
          entrypoint
          select
          fork
          path
          ;
        eval_entrypoint = r.evalEntrypoint;
      }) cfg.watch.repos;
    } cfg.settings
  );

  # Answers git's HTTPS credential requests for github.com with $GITHUB_TOKEN
  # (delivered through environmentFile), so update branches can be pushed.
  gitCredentialHelper = pkgs.writeShellScript "git-credential-github-token" ''
    [ "''${1:-}" = get ] || exit 0
    [ -n "''${GITHUB_TOKEN:-}" ] || exit 0
    cat > /dev/null
    echo username=x-access-token
    echo "password=$GITHUB_TOKEN"
  '';

  # Used as the services' system-level git config (GIT_CONFIG_SYSTEM): commit
  # identity plus the credential helper. A ~/.gitconfig in the state directory
  # still takes precedence.
  gitConfigFile = pkgs.writeText "ekapkgs-update-gitconfig" (
    lib.generators.toGitINI (
      {
        user = {
          name = cfg.git.userName;
          email = cfg.git.userEmail;
        };
      }
      // lib.optionalAttrs cfg.git.githubTokenCredentialHelper {
        credential."https://github.com".helper = "${gitCredentialHelper}";
      }
    )
  );

  servicePath = [ cfg.package ] ++ cfg.extraPackages;

  watchConfigFile = settingsFormat.generate "ekapkgs-update-watch.toml" watchEffectiveSettings;

  watchStartScript = pkgs.writeShellScript "ekapkgs-update-watch-start" ''
    set -eu

    runtime_env="$RUNTIME_DIRECTORY/env"
    : > "$runtime_env"
    chmod 600 "$runtime_env"

    ${lib.optionalString useCredential ''
      if [ -r "$CREDENTIALS_DIRECTORY/cachix-token" ]; then
        token="$(cat "$CREDENTIALS_DIRECTORY/cachix-token")"
        printf 'CACHIX_AUTH_TOKEN=%s\n' "$token" >> "$runtime_env"
      fi
    ''}

    set -a
    # shellcheck disable=SC1090
    . "$runtime_env"
    set +a

    exec ${cfg.package}/bin/ekapkgs-update watch \
      --config-file ${watchConfigFile} \
      --poll-interval ${toString cfg.watch.pollIntervalMinutes} \
      --index-refresh-hours ${toString cfg.watch.indexRefreshHours} \
      ${
        lib.optionalString (
          cfg.watch.commitStrategy != null
        ) "--commit-strategy ${cfg.watch.commitStrategy}"
      } \
      ${lib.escapeShellArgs cfg.watch.extraArgs}
  '';

  # Helper script: inject credentials into env, then exec the daemon.
  startScript = pkgs.writeShellScript "ekapkgs-update-start" ''
    set -eu

    runtime_env="$RUNTIME_DIRECTORY/env"
    : > "$runtime_env"
    chmod 600 "$runtime_env"

    ${lib.optionalString useCredential ''
      if [ -r "$CREDENTIALS_DIRECTORY/cachix-token" ]; then
        token="$(cat "$CREDENTIALS_DIRECTORY/cachix-token")"
        printf 'CACHIX_AUTH_TOKEN=%s\n' "$token" >> "$runtime_env"
      fi
    ''}

    # systemd will source this via EnvironmentFile=- (declared below).
    set -a
    # shellcheck disable=SC1090
    . "$runtime_env"
    set +a

    exec ${cfg.package}/bin/ekapkgs-update run \
      --config-file ${configFile} \
      ${
        lib.optionalString (
          cfg.cachix.cacheName != null
        ) "--cachix-cache ${lib.escapeShellArg cfg.cachix.cacheName}"
      } \
      ${lib.escapeShellArgs cfg.extraArgs}
  '';

  # ── Settings submodule types ──────────────────────────────────────────

  llmType = lib.types.submodule {
    freeformType = settingsFormat.type;
    options = {
      base_url = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "http://llm-server:8080";
        description = ''
          Base URL of the OpenAI-compatible API server used by the autofix
          pipeline. When `null`, autofix commands will look for the
          `EKAPKGS_LLM_BASE_URL` environment variable.
        '';
      };
      model = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "qwen2.5-coder:3b";
        description = ''
          Model name for chat completions and embeddings.
        '';
      };
      max_tokens = lib.mkOption {
        type = lib.types.nullOr lib.types.ints.positive;
        default = null;
        example = 2048;
        description = "Maximum tokens in LLM responses.";
      };
      temperature = lib.mkOption {
        type = lib.types.nullOr lib.types.float;
        default = null;
        example = 0.1;
        description = "Sampling temperature (0.0 = deterministic).";
      };
      timeout_secs = lib.mkOption {
        type = lib.types.nullOr lib.types.ints.positive;
        default = null;
        example = 300;
        description = "Request timeout in seconds for LLM calls.";
      };
    };
  };

  autofixType = lib.types.submodule {
    freeformType = settingsFormat.type;
    options = {
      max_attempts = lib.mkOption {
        type = lib.types.nullOr lib.types.ints.positive;
        default = null;
        example = 3;
        description = ''
          Maximum LLM fix attempts per package before escalating to human review.
        '';
      };
    };
  };

  nixBuildType = lib.types.submodule {
    freeformType = settingsFormat.type;
    options = {
      builders = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "ssh://builder";
        description = ''
          Nix builders string passed as `--builders` to `nix-build`.
        '';
      };
      max_jobs = lib.mkOption {
        type = lib.types.nullOr lib.types.ints.unsigned;
        default = null;
        example = 4;
        description = ''
          Maximum number of local build jobs passed as `--max-jobs` to
          `nix-build`. Set to 0 to force all builds to remote builders.
        '';
      };
    };
  };

  settingsType = lib.types.submodule {
    freeformType = settingsFormat.type;
    options = {
      llm = lib.mkOption {
        type = llmType;
        default = { };
        description = "LLM server configuration for the autofix pipeline.";
      };
      autofix = lib.mkOption {
        type = autofixType;
        default = { };
        description = "Autofix pipeline settings.";
      };
      nix = lib.mkOption {
        type = nixBuildType;
        default = { };
        description = "Nix build settings applied globally.";
      };
    };
  };

in
{
  options.services.ekapkgs-update = {
    enable = lib.mkEnableOption "the ekapkgs-update automated package-update daemon";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.ekapkgs-update;
      defaultText = lib.literalExpression "pkgs.ekapkgs-update";
      description = "The ekapkgs-update package to run.";
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "ekapkgs-update";
      description = ''
        System user the daemon runs as. The user is created automatically
        when this module is enabled.
      '';
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "ekapkgs-update";
      description = ''
        Group the daemon runs as. The group is created automatically when
        this module is enabled.
      '';
    };

    stateDirectory = lib.mkOption {
      type = lib.types.str;
      default = "ekapkgs-update";
      description = ''
        Subdirectory under <filename>/var/lib</filename> used as the
        daemon's state directory. Houses the SQLite database tracking
        update history.
      '';
    };

    packagesFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      example = lib.literalExpression "./default.nix";
      description = ''
        Path to the Nix file evaluated to discover packages to update. Used
        by the `ekapkgs-update run` service (required when
        {option}`services.ekapkgs-update.run.enable` is set) and as the
        default `--file` for other commands. Not needed for multi-repo watch
        mode, where each repository declares its own entry point.
      '';
    };

    run = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Whether to run the batch `ekapkgs-update run` service, which
          evaluates {option}`services.ekapkgs-update.packagesFile` and
          updates every outdated package. Disable it when the watch daemon
          already covers the same repositories.
        '';
      };

      startAt = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "daily";
        description = ''
          systemd calendar expression for running the batch service
          periodically. When `null`, it runs once at boot.
        '';
      };
    };

    extraPackages = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ ];
      example = lib.literalExpression "[ pkgs.claude-code ]";
      description = ''
        Extra packages on the `PATH` of the run and watch services.
        `--claude-fix` needs `pkgs.claude-code` here (installing it in
        `environment.systemPackages` does not make it visible to the
        services) plus credentials, e.g. `ANTHROPIC_API_KEY` or
        `CLAUDE_CODE_OAUTH_TOKEN` in {option}`services.ekapkgs-update.environmentFile`.
      '';
    };

    git = {
      userName = lib.mkOption {
        type = lib.types.str;
        default = "ekapkgs-update";
        description = "Author name for update commits.";
      };

      userEmail = lib.mkOption {
        type = lib.types.str;
        default = "noreply@ekapkgs.org";
        description = "Author email for update commits.";
      };

      githubTokenCredentialHelper = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Configure a git credential helper that authenticates HTTPS pushes
          to github.com with the `GITHUB_TOKEN` from
          {option}`services.ekapkgs-update.environmentFile`. Without it (or
          another credential setup), pushing update branches fails.
        '';
      };
    };

    environmentFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = ''
        Path to a file containing additional environment variables for
        the service, in the format `KEY=VALUE` per line.
        Useful for supplying `GITHUB_TOKEN`,
        `CACHIX_AUTH_TOKEN`, or
        `CACHIX_CACHE_NAME`.

        This is the recommended way to deliver secrets and is compatible
        with sops-nix, agenix, and other secret-management modules.

        See also {option}`services.ekapkgs-update.cachix.authTokenFile`
        for an alternative, credential-based delivery mechanism.
      '';
    };

    extraArgs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [
        "--skip-cve"
        "--max-rebuilds"
        "100"
      ];
      description = ''
        Additional command-line arguments passed to the
        `ekapkgs-update run` invocation.
      '';
    };

    cachix = {
      cacheName = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "ekapkgs";
        description = ''
          Name of the Cachix cache to push successful builds to. When set,
          the corresponding `--cachix-cache` CLI flag is
          appended automatically. Set to `null` to leave
          Cachix push disabled regardless of any token.
        '';
      };

      authTokenFile = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = ''
          Path to a file containing the raw Cachix auth token (no
          `KEY=` prefix, just the token value). When set,
          the file is loaded via systemd's `LoadCredential=`
          mechanism and exported as the `CACHIX_AUTH_TOKEN`
          environment variable to the service.

          Prefer {option}`environmentFile` if you already manage
          other secrets (`GITHUB_TOKEN` etc.) through
          a single sops-nix/agenix file; this option exists for
          deployments that want stricter credential sandboxing.
        '';
      };
    };

    settings = lib.mkOption {
      type = settingsType;
      default = { };
      description = ''
        Configuration for ekapkgs-update, serialised to
        `config.toml` and passed via `--config-file`.
        The submodule is freeform: any key not explicitly modelled here
        is still accepted and forwarded as-is to the TOML output.
      '';
    };

    web = {
      enable = lib.mkEnableOption "the ekapkgs-update web monitoring portal";

      package = lib.mkOption {
        type = lib.types.package;
        default = pkgs.ekapkgs-update-web or cfg.package;
        defaultText = lib.literalExpression "pkgs.ekapkgs-update-web";
        description = "The ekapkgs-update-web package to run.";
      };

      host = lib.mkOption {
        type = lib.types.str;
        default = "127.0.0.1";
        example = "0.0.0.0";
        description = ''
          Host address to bind the web server to. Use `0.0.0.0`
          to listen on all interfaces, or `127.0.0.1` for
          localhost only.
        '';
      };

      port = lib.mkOption {
        type = lib.types.port;
        default = 3000;
        example = 8080;
        description = "Port the web server listens on.";
      };

      cors = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          Enable CORS (Cross-Origin Resource Sharing) headers for public access.
          Enable this if you need to access the web portal from different domains
          or if you're serving it behind a reverse proxy.
        '';
      };

      user = lib.mkOption {
        type = lib.types.str;
        default = cfg.user;
        defaultText = lib.literalExpression "config.services.ekapkgs-update.user";
        description = ''
          System user the web portal runs as. Defaults to the same user as the
          main ekapkgs-update service to ensure database access.
        '';
      };

      group = lib.mkOption {
        type = lib.types.str;
        default = cfg.group;
        defaultText = lib.literalExpression "config.services.ekapkgs-update.group";
        description = ''
          Group the web portal runs as. Defaults to the same group as the
          main ekapkgs-update service.
        '';
      };

      database = lib.mkOption {
        type = lib.types.str;
        default = "/var/lib/${cfg.stateDirectory}/updates.db";
        defaultText = lib.literalExpression ''"/var/lib/''${config.services.ekapkgs-update.stateDirectory}/updates.db"'';
        description = ''
          Path to the SQLite database. Defaults to the same database used by
          the main ekapkgs-update service. The web portal has read-only access.
        '';
      };

      openFirewall = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          Whether to automatically open the firewall for the web portal port.
        '';
      };
    };

    watch = {
      enable = lib.mkEnableOption "the ekapkgs-update watch (passive/event-driven) daemon";

      pollIntervalMinutes = lib.mkOption {
        type = lib.types.ints.positive;
        default = 10;
        example = 5;
        description = ''
          How often (in minutes) to poll upstream RSS/Atom feeds and APIs for
          new releases.
        '';
      };

      indexRefreshHours = lib.mkOption {
        type = lib.types.ints.positive;
        default = 24;
        example = 12;
        description = ''
          How often (in hours) to rebuild the full upstream index via
          `nix-eval-jobs`. The index maps upstream sources to Nix attr_paths.
        '';
      };

      commitStrategy = lib.mkOption {
        type = lib.types.nullOr (
          lib.types.enum [
            "worktrees"
            "branch"
          ]
        );
        default = null;
        example = "worktrees";
        description = ''
          Strategy for committing updates. `worktrees` (default) uses isolated
          git worktrees and optionally creates PRs; `branch` commits directly
          to the current branch.
        '';
      };

      repos = lib.mkOption {
        type = lib.types.listOf (
          lib.types.submodule {
            options = {
              name = lib.mkOption {
                # Must match `config::is_valid_repo_name`: `+` separates the
                # repo from the attr path in worktree/failure directory names.
                type = lib.types.strMatching "[A-Za-z0-9][A-Za-z0-9_-]*";
                description = ''
                  Unique name for this repository: ASCII letters, digits, `-`
                  and `_`, starting with a letter or digit.
                '';
                example = "core-pkgs";
              };
              upstream = lib.mkOption {
                type = lib.types.str;
                description = ''
                  Upstream repo URL or `owner/repo` shorthand (assumes GitHub).
                  Used to derive PR target and for auto-cloning.
                '';
                example = "github.com/ekala-project/core-pkgs";
              };
              entrypoint = lib.mkOption {
                type = lib.types.str;
                default = "default.nix";
                description = ''
                  Nix file (relative to the repo root) enumerated with
                  `nix-eval-jobs` to discover packages.
                '';
              };
              select = lib.mkOption {
                type = lib.types.nullOr lib.types.str;
                default = null;
                example = "x: x.buildable";
                description = ''
                  `nix-eval-jobs --select` function applied to
                  `entrypoint`, for entry points that return nested sets
                  (e.g. `{ buildable = ...; }`) instead of the packages
                  themselves. Dotted keys in the selected set (`"cmake.v4"`)
                  are treated as attribute paths. Requires
                  `evalEntrypoint`.
                '';
              };
              evalEntrypoint = lib.mkOption {
                type = lib.types.nullOr lib.types.str;
                default = null;
                example = "default.nix";
                description = ''
                  Nix file (relative to the repo root) against which package
                  attribute paths are evaluated, located and built. Defaults
                  to `entrypoint`.
                '';
              };
              fork = lib.mkOption {
                type = lib.types.str;
                default = "origin";
                description = "Git remote name to push update branches to.";
              };
              path = lib.mkOption {
                type = lib.types.str;
                description = "Local path to the repo checkout. Auto-cloned if missing.";
                example = "/var/lib/ekapkgs-update/repos/core-pkgs";
              };
            };
          }
        );
        default = [ ];
        description = ''
          List of repositories to watch for updates. When non-empty, the
          watch daemon builds a per-repo upstream index and routes update PRs
          to the correct upstream repository. Each repo is auto-cloned if the
          checkout does not exist.
        '';
        example = lib.literalExpression ''
          [
            {
              name = "corepkgs";
              upstream = "github.com/ekala-project/corepkgs";
              entrypoint = "ci/packages.nix";
              select = "x: x.buildable";
              evalEntrypoint = "default.nix";
              path = "/var/lib/ekapkgs-update/repos/corepkgs";
            }
            {
              name = "ekapkgs";
              upstream = "github.com/ekala-project/ekapkgs";
              entrypoint = "ci/repos-packages.nix";
              select = "x: x.buildable";
              evalEntrypoint = "default.nix";
              path = "/var/lib/ekapkgs-update/repos/ekapkgs";
            }
          ]
        '';
      };

      extraArgs = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        example = [
          "--dry-run"
          "--preserve-failures"
        ];
        description = ''
          Additional command-line arguments passed to the
          `ekapkgs-update watch` invocation.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion =
          cfg.cachix.cacheName != null -> (cfg.environmentFile != null || cfg.cachix.authTokenFile != null);
        message = ''
          services.ekapkgs-update.cachix.cacheName is set but no auth-token
          source is configured. Set either
          services.ekapkgs-update.environmentFile (containing
          CACHIX_AUTH_TOKEN=...) or services.ekapkgs-update.cachix.authTokenFile.
        '';
      }
      {
        assertion =
          let
            names = map (r: r.name) cfg.watch.repos;
          in
          names == lib.unique names;
        message = ''
          services.ekapkgs-update.watch.repos: repo names must be unique.
        '';
      }
      {
        assertion = lib.all (r: lib.toLower r.name != "default") cfg.watch.repos;
        message = ''
          services.ekapkgs-update.watch.repos: "default" is reserved (the
          dashboard uses it for the single-repo / `run` scope).
        '';
      }
      {
        assertion = cfg.run.enable -> cfg.packagesFile != null;
        message = ''
          services.ekapkgs-update.run.enable requires
          services.ekapkgs-update.packagesFile. Set it, or set
          services.ekapkgs-update.run.enable = false when only the watch
          daemon is used.
        '';
      }
    ]
    ++ map (r: {
      assertion = r.select != null -> r.evalEntrypoint != null;
      message = ''
        services.ekapkgs-update.watch.repos (${r.name}): `select` requires
        `evalEntrypoint` (e.g. "default.nix"), because attribute paths from the
        selected set must be evaluated against a file where they resolve.
      '';
    }) cfg.watch.repos;

    nixpkgs.overlays = [
      (import ./overlay.nix)
    ];

    users.users = lib.mkIf (cfg.user == "ekapkgs-update") {
      ekapkgs-update = {
        isSystemUser = true;
        group = cfg.group;
        home = "/var/lib/${cfg.stateDirectory}";
        createHome = false; # systemd StateDirectory= handles this
        description = "ekapkgs-update daemon user";
      };
    };

    users.groups = lib.mkIf (cfg.group == "ekapkgs-update") {
      ekapkgs-update = { };
    };

    systemd.services.ekapkgs-update = lib.mkIf cfg.run.enable {
      description = "ekapkgs-update automated package update daemon";
      # With startAt the timer starts the service; otherwise run once at boot.
      wantedBy = lib.optional (cfg.run.startAt == null) "multi-user.target";
      startAt = lib.mkIf (cfg.run.startAt != null) cfg.run.startAt;
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];

      # The wrapped package puts nix/git/gh/cachix on PATH, but several
      # subprocesses inspect this anyway; setting it explicitly keeps
      # subprocess output predictable in journalctl.
      path = servicePath;

      environment.GIT_CONFIG_SYSTEM = "${gitConfigFile}";

      serviceConfig = {
        Type = "simple";
        ExecStart = "${startScript}";
        Restart = "on-failure";
        RestartSec = "30s";

        User = cfg.user;
        Group = cfg.group;

        StateDirectory = cfg.stateDirectory;
        StateDirectoryMode = "0750";
        RuntimeDirectory = "ekapkgs-update";
        RuntimeDirectoryMode = "0700";
        WorkingDirectory = "/var/lib/${cfg.stateDirectory}";

        EnvironmentFile = lib.optional (cfg.environmentFile != null) cfg.environmentFile;

        LoadCredential = lib.optional useCredential "cachix-token:${toString cfg.cachix.authTokenFile}";

        # Hardening — Nix builds need broad filesystem access, so we
        # apply the conservative subset that is compatible with invoking
        # `nix-build` from within the unit.
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;

        # Allow read/write to the Nix daemon socket and the local store.
        ReadWritePaths = [
          "/nix/var/nix/daemon-socket"
          "/nix/var/nix/profiles/per-user"
          "/nix/var/nix/gcroots/per-user"
        ];
      };
    };

    # Web portal service
    systemd.services.ekapkgs-update-web = lib.mkIf cfg.web.enable {
      description = "ekapkgs-update web monitoring portal";
      wantedBy = [ "multi-user.target" ];
      after = [
        "network-online.target"
        "ekapkgs-update.service"
      ];
      wants = [ "network-online.target" ];

      serviceConfig = {
        Type = "simple";
        ExecStart = lib.escapeShellArgs (
          [
            "${cfg.web.package}/bin/ekapkgs-update-web"
            "--database"
            cfg.web.database
            "--host"
            cfg.web.host
            "--port"
            (toString cfg.web.port)
          ]
          ++ lib.optional cfg.web.cors "--cors"
        );

        Restart = "on-failure";
        RestartSec = "10s";

        User = cfg.web.user;
        Group = cfg.web.group;

        # Hardening - web portal is read-only and doesn't need much access
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        RestrictRealtime = true;
        RestrictNamespaces = true;
        SystemCallArchitectures = "native";

        ReadWritePaths = [ "/var/lib/${cfg.stateDirectory}" ];
      };
    };

    # Watch (passive/event-driven) service
    systemd.services.ekapkgs-update-watch = lib.mkIf cfg.watch.enable {
      description = "ekapkgs-update watch daemon (RSS/event-driven updates)";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];

      path = servicePath;

      environment.GIT_CONFIG_SYSTEM = "${gitConfigFile}";

      serviceConfig = {
        Type = "simple";
        ExecStart = "${watchStartScript}";
        Restart = "always";
        RestartSec = "60s";

        User = cfg.user;
        Group = cfg.group;

        StateDirectory = cfg.stateDirectory;
        StateDirectoryMode = "0750";
        RuntimeDirectory = "ekapkgs-update-watch";
        RuntimeDirectoryMode = "0700";
        WorkingDirectory = "/var/lib/${cfg.stateDirectory}";

        EnvironmentFile = lib.optional (cfg.environmentFile != null) cfg.environmentFile;

        LoadCredential = lib.optional useCredential "cachix-token:${toString cfg.cachix.authTokenFile}";

        # Same hardening as the main daemon — needs Nix daemon access
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;

        ReadWritePaths = [
          "/nix/var/nix/daemon-socket"
          "/nix/var/nix/profiles/per-user"
          "/nix/var/nix/gcroots/per-user"
        ];
      };
    };

    # Firewall configuration
    networking.firewall.allowedTCPPorts = lib.mkIf (cfg.web.enable && cfg.web.openFirewall) [
      cfg.web.port
    ];
  };

  meta.maintainers = [ ];
}
