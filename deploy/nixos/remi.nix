# deploy/nixos/remi.nix
# NixOS module for Conary Remi server
#
# To deploy on Hetzner Cloud:
# 1. Create a CX32 instance with Ubuntu 24.04 or Debian 12
# 2. Run nixos-infect:
#    curl https://raw.githubusercontent.com/elitak/nixos-infect/master/nixos-infect \
#      | NIX_CHANNEL=nixos-24.11 bash -x
# 3. Add this module to /etc/nixos/configuration.nix:
#    imports = [ ./remi.nix ];
#    services.conary-remi.enable = true;
# 4. nixos-rebuild switch
{ config, lib, pkgs, ... }:

let
  cfg = config.services.conary-remi;
in {
  options.services.conary-remi = {
    enable = lib.mkEnableOption "Conary Remi CCS conversion proxy server";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./conary.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ./conary.nix { }";
      description = ''
        The Conary package to use. Must be built with `--features server`.
      '';
    };

    configFile = lib.mkOption {
      type = lib.types.path;
      default = "/etc/conary/remi.toml";
      description = ''
        Path to the Remi server TOML configuration file.
        See deploy/remi.toml.example for a complete reference.
      '';
    };

    storageRoot = lib.mkOption {
      type = lib.types.path;
      default = "/conary";
      description = ''
        Root directory for all Remi storage (chunks, metadata, cache, etc.).
        Should be on a dedicated partition or ZFS dataset for production use.
      '';
    };

    webRoot = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = ''
        Path to the built SvelteKit web frontend directory.
        Set to null to disable the web frontend.
      '';
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "conary";
      description = "User account under which the Remi server runs.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "conary";
      description = "Group under which the Remi server runs.";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Whether to open TCP ports 80 and 443 in the firewall.";
    };

    logLevel = lib.mkOption {
      type = lib.types.str;
      default = "conary=info,tower_http=info";
      description = ''
        Rust log level filter string (RUST_LOG format).
        Examples: "conary=debug", "conary=info,tower_http=warn"
      '';
    };

    r2 = {
      accessKeyFile = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = ''
          Path to a file containing the Cloudflare R2 access key.
          The file should contain only the key, with no trailing newline.
          Permissions should be 0400 owned by root.
        '';
      };

      secretKeyFile = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = ''
          Path to a file containing the Cloudflare R2 secret key.
          The file should contain only the key, with no trailing newline.
          Permissions should be 0400 owned by root.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable {
    # Create service user and group
    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      home = cfg.storageRoot;
      description = "Conary Remi server";
    };

    users.groups.${cfg.group} = { };

    # Ensure storage directories exist
    systemd.tmpfiles.rules = [
      "d ${cfg.storageRoot}           0750 ${cfg.user} ${cfg.group} -"
      "d ${cfg.storageRoot}/chunks    0750 ${cfg.user} ${cfg.group} -"
      "d ${cfg.storageRoot}/converted 0750 ${cfg.user} ${cfg.group} -"
      "d ${cfg.storageRoot}/built     0750 ${cfg.user} ${cfg.group} -"
      "d ${cfg.storageRoot}/bootstrap 0750 ${cfg.user} ${cfg.group} -"
      "d ${cfg.storageRoot}/build     0750 ${cfg.user} ${cfg.group} -"
      "d ${cfg.storageRoot}/metadata  0750 ${cfg.user} ${cfg.group} -"
      "d ${cfg.storageRoot}/manifests 0750 ${cfg.user} ${cfg.group} -"
      "d ${cfg.storageRoot}/keys      0700 ${cfg.user} ${cfg.group} -"
      "d ${cfg.storageRoot}/cache     0750 ${cfg.user} ${cfg.group} -"
      "d /etc/conary                  0755 root        root         -"
    ];

    # Main systemd service
    systemd.services.conary-remi = {
      description = "Conary Remi Package Server";
      documentation = [ "https://github.com/ConaryLabs/Conary" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];

      environment = {
        RUST_LOG = cfg.logLevel;
        RUST_BACKTRACE = "1";
      };

      # Load R2 credentials from files if configured
      script = let
        loadR2 = lib.optionalString (cfg.r2.accessKeyFile != null && cfg.r2.secretKeyFile != null) ''
          export CONARY_R2_ACCESS_KEY="$(< ${cfg.r2.accessKeyFile})"
          export CONARY_R2_SECRET_KEY="$(< ${cfg.r2.secretKeyFile})"
        '';
      in ''
        ${loadR2}
        exec ${cfg.package}/bin/conary remi --config ${cfg.configFile}
      '';

      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        Group = cfg.group;
        Restart = "always";
        RestartSec = 5;
        WatchdogSec = 120;

        # Resource limits
        LimitNOFILE = 65536;
        LimitNPROC = 4096;

        # Security hardening
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        MemoryDenyWriteExecute = true;
        LockPersonality = true;
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;

        # Allow access to storage and config
        ReadWritePaths = [ cfg.storageRoot ];
        ReadOnlyPaths = [
          "/etc/conary"
        ] ++ lib.optional (cfg.r2.accessKeyFile != null) cfg.r2.accessKeyFile
          ++ lib.optional (cfg.r2.secretKeyFile != null) cfg.r2.secretKeyFile;

        # Network
        RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];

        # Logging
        StandardOutput = "journal";
        StandardError = "journal";
        SyslogIdentifier = "conary-remi";
      };
    };

    # Open firewall ports when requested
    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [ 80 443 ];
  };
}
