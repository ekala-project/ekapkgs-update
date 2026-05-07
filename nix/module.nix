{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.ekapkgs-update;

  # The Cachix auth token can be supplied two ways:
  #
  # - environmentFile  → a path containing `CACHIX_AUTH_TOKEN=...` (and
  #                      optionally GITHUB_TOKEN, etc.). Loaded by systemd
  #                      via `EnvironmentFile=`. This is the default and
  #                      composes cleanly with sops-nix / agenix.
  #
  # - cachix.authTokenFile → a path containing the bare token value (no
  #                          `KEY=` prefix). Loaded via systemd
  #                          `LoadCredential=` and exported into the
  #                          environment by an ExecStartPre helper. Use
  #                          this for stricter sandboxing.
  useCredential = cfg.cachix.authTokenFile != null;

  # Helper script: read the credential file (if any), append it to a
  # runtime env file, then exec the daemon. We can't use only
  # `EnvironmentFile=` for credentials because the credentials directory is
  # only available inside the unit, not at unit-load time.
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
      --file ${lib.escapeShellArg cfg.packagesFile} \
      --database ${lib.escapeShellArg "/var/lib/${cfg.stateDirectory}/updates.db"} \
      ${
        lib.optionalString (
          cfg.cachix.cacheName != null
        ) "--cachix-cache ${lib.escapeShellArg cfg.cachix.cacheName}"
      } \
      ${lib.escapeShellArgs cfg.extraArgs}
  '';
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
      default = "eka-ci";
      description = ''
        System user the daemon runs as. The user is created automatically
        when this module is enabled.
      '';
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "eka-ci";
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
      type = lib.types.path;
      example = lib.literalExpression "./default.nix";
      description = ''
        Path to the Nix file evaluated to discover packages to update.
      '';
    };

    environmentFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = ''
        Path to a file containing additional environment variables for
        the service, in the format <literal>KEY=VALUE</literal> per line.
        Useful for supplying <literal>GITHUB_TOKEN</literal>,
        <literal>CACHIX_AUTH_TOKEN</literal>, or
        <literal>CACHIX_CACHE_NAME</literal>.

        This is the recommended way to deliver secrets and is compatible
        with sops-nix, agenix, and other secret-management modules.

        See also <option>services.ekapkgs-update.cachix.authTokenFile</option>
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
        <literal>ekapkgs-update run</literal> invocation.
      '';
    };

    cachix = {
      cacheName = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "ekapkgs";
        description = ''
          Name of the Cachix cache to push successful builds to. When set,
          the corresponding <literal>--cachix-cache</literal> CLI flag is
          appended automatically. Set to <literal>null</literal> to leave
          Cachix push disabled regardless of any token.
        '';
      };

      authTokenFile = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = ''
          Path to a file containing the raw Cachix auth token (no
          <literal>KEY=</literal> prefix, just the token value). When set,
          the file is loaded via systemd's <literal>LoadCredential=</literal>
          mechanism and exported as the <literal>CACHIX_AUTH_TOKEN</literal>
          environment variable to the service.

          Prefer <option>environmentFile</option> if you already manage
          other secrets (<literal>GITHUB_TOKEN</literal> etc.) through
          a single sops-nix/agenix file; this option exists for
          deployments that want stricter credential sandboxing.
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
    ];

    users.users = lib.mkIf (cfg.user == "eka-ci") {
      eka-ci = {
        isSystemUser = true;
        group = cfg.group;
        home = "/var/lib/${cfg.stateDirectory}";
        createHome = false; # systemd StateDirectory= handles this
        description = "ekapkgs-update CI daemon user";
      };
    };

    users.groups = lib.mkIf (cfg.group == "eka-ci") {
      eka-ci = { };
    };

    systemd.services.ekapkgs-update = {
      description = "ekapkgs-update automated package update daemon";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];

      # The wrapped package puts nix/git/gh/cachix on PATH, but several
      # subprocesses inspect this anyway; setting it explicitly keeps
      # subprocess output predictable in journalctl.
      path = [ cfg.package ];

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
  };

  meta.maintainers = [ ];
}
