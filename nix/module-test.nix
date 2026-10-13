# Evaluates the Home Manager module against stubs for the option paths it reads and writes,
# so the wiring is checked without home-manager as an input. Produces what each setup would
# emit; flake.nix turns it into assertions
{
  lib,
  pkgs,
  module,
}:

let
  # Home Manager adds `lib.hm`; the stub keeps the shape the module reads
  hmLib = lib.extend (
    _: _: {
      hm.dag.entryAfter = after: data: { inherit after data; };
    }
  );

  stubs =
    { lib, ... }:
    {
      options = {
        home.packages = lib.mkOption {
          type = lib.types.listOf lib.types.package;
          default = [ ];
        };
        home.activation = lib.mkOption {
          type = lib.types.attrsOf lib.types.attrs;
          default = { };
        };
        systemd.user.services = lib.mkOption {
          type = lib.types.attrsOf lib.types.attrs;
          default = { };
        };
        xdg.configHome = lib.mkOption {
          type = lib.types.str;
          default = "/home/u/.config";
        };
        wayland.windowManager.hyprland = {
          enable = lib.mkEnableOption "Hyprland";
          configType = lib.mkOption {
            type = lib.types.str;
            default = "hyprlang";
          };
          extraConfig = lib.mkOption {
            type = lib.types.lines;
            default = "";
          };
        };
      };
    };

  eval =
    user:
    (lib.evalModules {
      modules = [
        stubs
        module
        user
      ];
      specialArgs = {
        inherit pkgs;
        lib = hmLib;
      };
    }).config;

  luaHyprland = {
    wayland.windowManager.hyprland = {
      enable = true;
      configType = "lua";
    };
  };

  summary = config: {
    # The name is enough here: with its string context the check would build molva first
    packages = map (p: builtins.unsafeDiscardStringContext (toString p)) config.home.packages;
    extraConfig = config.wayland.windowManager.hyprland.extraConfig;
    activation = config.home.activation.molvaiHyprland.data or "";
    daemon = builtins.unsafeDiscardStringContext (
      config.systemd.user.services.molva-daemon.Service.ExecStart or ""
    );
  };
in
{
  lua = summary (eval (luaHyprland // { programs.molvai.enable = true; }));
  hyprlang = summary (eval {
    programs.molvai.enable = true;
    wayland.windowManager.hyprland.enable = true;
  });
  optedOut = summary (
    eval (
      luaHyprland
      // {
        programs.molvai = {
          enable = true;
          hyprland.enable = false;
        };
      }
    )
  );
  off = summary (eval luaHyprland);
  noDaemon = summary (eval {
    programs.molvai = {
      enable = true;
      daemon.enable = false;
    };
  });
}
