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
        assertions = lib.mkOption {
          type = lib.types.listOf lib.types.attrs;
          default = [ ];
        };
        systemd.user.services = lib.mkOption {
          type = lib.types.attrsOf lib.types.attrs;
          default = { };
        };
        xdg.configHome = lib.mkOption {
          type = lib.types.str;
          default = "/home/u/.config";
        };
        xdg.configFile = lib.mkOption {
          type = lib.types.attrsOf lib.types.attrs;
          default = { };
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
    activation = builtins.unsafeDiscardStringContext (config.home.activation.molvaiHyprland.data or "");
    daemon = builtins.unsafeDiscardStringContext (
      config.systemd.user.services.molva-daemon.Service.ExecStart or ""
    );
    guiAutostart = config.xdg.configFile."autostart/molvai-gui.desktop".text or "";
    model = config.programs.molvai.model;
    models = config.programs.molvai.models;
    configSource = builtins.unsafeDiscardStringContext (
      toString (config.xdg.configFile."molva/config.toml".source or "")
    );
    binds = builtins.unsafeDiscardStringContext (
      toString (config.xdg.configFile."hypr/molvai.lua".source or "")
    );
    settings = config.programs.molvai.settings;
  };

  expectAutostart =
    expected: user:
    let
      result = summary (eval user);
    in
    assert lib.assertMsg (builtins.elem "Exec=${expected}" (
      lib.splitString "\n" result.guiAutostart
    )) "GUI autostart Exec must preserve the command and append --autostart";
    result;

  customCommand = ''"/opt/MolvAI GUI/molva-gui" --config "/home/u/config file.toml" --label "100%%" %f'';
  guiPackage = pkgs.runCommand "module-test-gui" { meta.mainProgram = "molva-gui"; } "touch $out";
in
{
  lua = expectAutostart "molva-gui --autostart" (luaHyprland // { programs.molvai.enable = true; });
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
  noGuiAutostart = summary (eval {
    programs.molvai = {
      enable = true;
      guiAutostart.enable = false;
    };
  });
  customGuiCommand = expectAutostart "/run/current-system/sw/bin/molva-gui --autostart" {
    programs.molvai = {
      enable = true;
      guiAutostart.command = "/run/current-system/sw/bin/molva-gui";
    };
  };
  quotedGuiCommand = expectAutostart "${customCommand} --autostart" {
    programs.molvai = {
      enable = true;
      inherit guiPackage;
      guiAutostart.command = customCommand;
    };
  };
  packagedGui = expectAutostart "${lib.getExe guiPackage} --autostart" {
    programs.molvai = {
      enable = true;
      inherit guiPackage;
    };
  };
  guiWithoutDaemon = expectAutostart "molva-gui --autostart" {
    programs.molvai = {
      enable = true;
      daemon.enable = false;
      guiAutostart.enable = true;
    };
  };
  whisper = summary (eval {
    programs.molvai = {
      enable = true;
      model = "small";
      models = [ "small" ];
      transcribeCli = null;
      settings.stt.chunk_pause_ms = 400;
    };
  });
}
