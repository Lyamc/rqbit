# Adds a network-namespace option to the NixOS rqbit service.
# Nixpkgs already defines services.rqbit. Importing this file does not
# replace that module.
{ config, lib, ... }:

let
  cfg = config.services.rqbit;
in
{
  options.services.rqbit = {
    networkNamespace = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "nordvpn";
      description = ''
        Join this existing network namespace (`/run/netns/<name>`).
        rqbit then uses only that namespace's routes and
        `/etc/netns/<name>/resolv.conf`. Other services keep the host route.
      '';
    };

    namespaceService = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "nordvpn-netns.service";
      description = "systemd unit that creates `networkNamespace`. rqbit waits for it and stops with it.";
    };
  };

  config = lib.mkIf (cfg.enable && cfg.networkNamespace != null) {
    systemd.services.rqbit = {
      after = lib.optional (cfg.namespaceService != null) cfg.namespaceService;
      bindsTo = lib.optional (cfg.namespaceService != null) cfg.namespaceService;
      serviceConfig = {
        NetworkNamespacePath = "/run/netns/${cfg.networkNamespace}";
        BindReadOnlyPaths = [
          "/etc/netns/${cfg.networkNamespace}/resolv.conf:/etc/resolv.conf:norbind"
          "/etc/netns/${cfg.networkNamespace}/nsswitch.conf:/etc/nsswitch.conf:norbind"
        ];
        InaccessiblePaths = [ "-/run/nscd/socket" ];
      };
    };
  };
}
