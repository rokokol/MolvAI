# Home Manager module: installs the molva CLI, runs its daemon as a user service and, under a
# Hyprland with a Lua config, binds the hotkeys from MolvAI's own settings there
{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:

# The binds file is written by `molva setup hyprland --write` on each switch, not generated in
# the store: the GUI rewrites it when the hotkeys change, and a store link in its place would
# turn the next switch into a "file in the way" error. Home Manager only owns the line that
# includes the file, guarded so a Hyprland without the file still starts
let
  cfg = config.programs.molvai;
  hyprland = config.wayland.windowManager.hyprland;
  # `configType` arrived with Hyprland's Lua config; an older Home Manager has no such option,
  # and its hyprlang config would break on a Lua line
  hyprlandLua = (hyprland.enable or false) && (hyprland.configType or "hyprlang") == "lua";
  bindsFile = "${config.xdg.configHome}/hypr/molvai.lua";
in
{
  options.programs.molvai = {
    enable = lib.mkEnableOption "MolvAI, system-wide voice input";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.molva;
      defaultText = lib.literalExpression "the molva package of this flake";
      description = "The package that provides the molva command";
    };

    daemon.enable = lib.mkOption {
      type = lib.types.bool;
      default = pkgs.stdenv.hostPlatform.isLinux;
      defaultText = lib.literalExpression "pkgs.stdenv.hostPlatform.isLinux";
      description = ''
        Run `molva daemon` as a user service of the graphical session. The hotkeys only call
        `molva record`, which asks the daemon, so without it they report that the daemon is
        not running. The MolvAI GUI sees a running daemon and does not start a second one
      '';
    };

    hyprland.enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Bind the hotkeys from MolvAI's settings in Hyprland: each switch writes
        {file}`$XDG_CONFIG_HOME/hypr/molvai.lua` and hyprland.lua includes it. It takes effect
        only with `wayland.windowManager.hyprland.configType = "lua"`
      '';
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      { home.packages = [ cfg.package ]; }

      (lib.mkIf cfg.daemon.enable {
        systemd.user.services.molva-daemon = {
          Unit = {
            Description = "MolvAI daemon: the microphone, speech recognition and the paste";
            # The paste needs the compositor, so the daemon lives and dies with the session
            PartOf = [ "graphical-session.target" ];
            After = [ "graphical-session.target" ];
          };
          Service = {
            ExecStart = "${lib.getExe cfg.package} daemon";
            Restart = "on-failure";
            RestartSec = 5;
          };
          Install.WantedBy = [ "graphical-session.target" ];
        };
      })

      (lib.mkIf (cfg.hyprland.enable && hyprlandLua) {
        wayland.windowManager.hyprland.extraConfig = lib.mkAfter ''
          -- MolvAI hotkeys: molva writes this file from its settings
          local molvai = ${lib.generators.toLua { } bindsFile}
          local molvai_file = io.open(molvai, "r")
          if molvai_file then
            molvai_file:close()
            dofile(molvai)
          end
        '';

        # A failure must not stop the switch: the hotkeys are one feature, the generation is
        # the whole home
        home.activation.molvaiHyprland = lib.hm.dag.entryAfter [ "writeBoundary" ] ''
          run env XDG_CONFIG_HOME=${lib.escapeShellArg config.xdg.configHome} \
            ${lib.getExe cfg.package} setup hyprland --write \
            || warnEcho "MolvAI: ${bindsFile} was not written, see the message above"
        '';
      })
    ]
  );
}
