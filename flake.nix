{
  description = "MolvAI — open-source system-wide voice input (an analogue of Wispr Flow)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.transcribe-cpp = {
    url = "github:handy-computer/transcribe.cpp/v0.4.1";
    flake = false;
  };

  outputs =
    {
      self,
      nixpkgs,
      transcribe-cpp,
    }:
    let
      # The source of the GigaAM runtime; the package that builds it is below
      transcribeSource = transcribe-cpp;
      # No x86_64-darwin: Nixpkgs 26.05 is its last release, and the release workflow builds
      # macOS for Apple silicon only
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      # `nix run github:rokokol/MolvAI` builds and runs the CLI with one command,
      # without a clone and without a toolchain installed in the system
      packages = forAll (
        pkgs:
        let
          isLinux = pkgs.stdenv.hostPlatform.isLinux;
          # Only what the builds and `cargo test` read. The search_docs index is built into
          # the binary, so the README, docs and site are part of the source too: a
          # documentation edit rebuilds the binary, which is the price of a build-time index
          # with no second copy. The list is inclusive, not exclusive: a forgotten source
          # breaks the build loudly
          source = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./crates
              ./models.toml
              ./README.md
              ./docs
              ./site
              # The tests of the crates read fixtures and golden files from
              # CARGO_MANIFEST_DIR/../../tests, so they are part of the source
              ./tests/fixtures
              ./tests/golden
              ./rust-toolchain.toml
              ./rustfmt.toml
              ./clippy.toml
            ];
          };
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
          # npmDepsHash pins the UI lockfile; the package receives it from here
          npmDepsHash = "sha256-vyf8I1Ej0PC1gdWFDVxHc9ug1jyqp3fgAE4NhK6oWZA=";
          molva =
            pkgs.rustPlatform.buildRustPackage {
              pname = "molva";
              inherit version;
              src = source;
              cargoLock.lockFile = ./Cargo.lock;
              cargoBuildFlags = [
                "--bin"
                "molva"
              ];
              # By default, checkPhase tests the whole workspace, but molva-gui needs GTK and
              # webkit. Those are not in the buildInputs of the CLI, and must not be there. An
              # exclusion, not a list: a new crate without a graphical stack enters the tests by
              # itself
              cargoTestFlags = [
                "--workspace"
                "--exclude"
                "molva-gui"
              ];
              # The sandbox has no system TLS roots, and the HTTP client does not build at all.
              # The test of an unreachable provider would then check a broken TLS, not the
              # network. The cacert hook sets SSL_CERT_FILE, and rustls-platform-verifier reads it
              nativeCheckInputs = with pkgs; [ cacert ];

              nativeBuildInputs = with pkgs; [
                cmake
                installShellFiles
                pkg-config
              ];
              # The completion command reads only the clap definition, so it can run in the
              # build sandbox. Bash, zsh and fish are the three shells the package installs
              postInstall = ''
                installShellCompletion --cmd molva \
                  --bash <($out/bin/molva completions bash) \
                  --zsh <($out/bin/molva completions zsh) \
                  --fish <($out/bin/molva completions fish)
              '';
              buildInputs =
                with pkgs;
                [ openssl ]
                ++ pkgs.lib.optionals isLinux [
                  alsa-lib
                  libxkbcommon
                  wayland
                ];

              # The whisper-rs bindings are in the crate, so the build does not need libclang
              WHISPER_DONT_GENERATE_BINDINGS = "1";
              # A reproducible build without -march=native, but with SIMD: x86-64-v3
              # (AVX2/FMA/F16C). Otherwise whisper.cpp builds scalar under Nix and runs 20
              # times slower
              GGML_NATIVE = "OFF";
            }
            // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isx86_64 {
              GGML_AVX = "ON";
              GGML_AVX2 = "ON";
              GGML_FMA = "ON";
              GGML_F16C = "ON";
            }
            // {

              meta = {
                description = "System-wide voice input with processing on your own computer";
                homepage = "https://github.com/rokokol/MolvAI";
                license = pkgs.lib.licenses.mit;
                mainProgram = "molva";
              };
            };
        in
        {
          inherit molva;
          molva-gui = pkgs.callPackage ./nix/molva-gui.nix {
            inherit source version npmDepsHash;
          };
          transcribe-cpp = pkgs.callPackage ./nix/transcribe-cpp.nix {
            src = transcribeSource;
          };
          default = molva;
        }
      );

      devShells = forAll (
        pkgs:
        let
          isLinux = pkgs.stdenv.hostPlatform.isLinux;

          # The three tools that decide what they can of the Nix: layout, antipatterns and dead
          # code. The travelling checker needs jq for its own rules and refuses to run without
          # every one of them, so the shell is where they live rather than the host
          nixTools = with pkgs; [
            nixfmt
            statix
            deadnix
            jq
          ];

          # The Rust toolchain and the tools of the gate
          # The lock moved to a nixpkgs where the cargo-llvm-cov tests fail inside the
          # sandbox while the tool itself builds. The check is what breaks the shell, so it is
          # skipped here and the coverage recipe still gets the binary
          cargoLlvmCov = pkgs.cargo-llvm-cov.overrideAttrs (_: {
            doCheck = false;
          });

          rustTools = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            cargo-tauri
            cargo-deny
            cargo-about
            cargoLlvmCov
            cargo-audit
            cargo-machete
            cargo-cyclonedx
            just
          ];

          # The build of whisper.cpp and the native dependencies
          nativeBuild = with pkgs; [
            cmake
            pkg-config
            nodejs
            # The comment checker reads syntax trees; the pinned pack carries every grammar
            (python3.withPackages (ps: [ ps.tree-sitter-language-pack ]))
          ];

          # The libraries of the core
          coreLibs = with pkgs; [ openssl ] ++ pkgs.lib.optionals isLinux [ alsa-lib ];

          # WebKitGTK decodes media through GStreamer, and the plugins are not a dependency of the
          # library: without them an H.264 file makes the window wait forever. The same list the
          # packaged window carries
          gstPlugins =
            with pkgs.gst_all_1;
            pkgs.lib.optionals isLinux [
              gstreamer
              gst-plugins-base
              gst-plugins-good
              gst-plugins-bad
              gst-libav
            ];

          # Tauri 2 on Linux: webkit, gtk, tray
          tauriLibs =
            with pkgs;
            pkgs.lib.optionals isLinux [
              webkitgtk_4_1
              libsoup_3
              gtk3
              glib
              librsvg
              libayatana-appindicator
              dbus
              wayland
              libxkbcommon
            ];

          # The tools for the demo on Wayland
          waylandTools =
            with pkgs;
            pkgs.lib.optionals isLinux [
              wtype
              wl-clipboard
            ];

          # The CUDA toolkit is unfree: a separate instance of nixpkgs with allowUnfree only for
          # the cuda shell
          pkgsUnfree = import nixpkgs {
            inherit (pkgs.stdenv.hostPlatform) system;
            config.allowUnfree = true;
          };

          base = {
            packages =
              rustTools
              ++ nixTools
              ++ nativeBuild
              ++ coreLibs
              ++ tauriLibs
              ++ waylandTools
              ++ gstPlugins;

            # WebKit reads the media plugins from this path and never from the registry
            GST_PLUGIN_SYSTEM_PATH_1_0 = pkgs.lib.optionalString isLinux (
              pkgs.lib.makeSearchPathOutput "lib" "lib/gstreamer-1.0" gstPlugins
            );

            # The whisper-rs bindings are already in the crate, so libclang is not needed
            WHISPER_DONT_GENERATE_BINDINGS = "1";
            # Nix sets SOURCE_DATE_EPOCH, and because of it ggml turns GGML_NATIVE off:
            # whisper.cpp builds without AVX2/FMA and runs 20 times slower. For the devShell, build
            # for your own machine.
            GGML_NATIVE = "ON";
            RUST_BACKTRACE = "1";
            # NVIDIA + Wayland: without this the webkit window stays empty
            WEBKIT_DISABLE_DMABUF_RENDERER = "1";

            shellHook = pkgs.lib.optionalString isLinux ''
              export XDG_DATA_DIRS="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}:$XDG_DATA_DIRS"
              # The GTK file chooser reads GSettings before it draws; the schemas are not in
              # the shell's default data path on every host
              export GSETTINGS_SCHEMA_DIR="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}/glib-2.0/schemas"
            '';
          };
        in
        {
          default = pkgs.mkShell base;

          # The build of whisper.cpp with CUDA (cargo build --features cuda).
          # On NixOS the NVIDIA driver is in /run/opengl-driver/lib, and the linker looks there
          # too (-lcuda).
          cuda = pkgs.mkShell (
            base
            // {
              packages = base.packages ++ pkgs.lib.optionals isLinux [ pkgsUnfree.cudaPackages.cudatoolkit ];
              CUDA_PATH = pkgs.lib.optionalString isLinux "${pkgsUnfree.cudaPackages.cudatoolkit}";
              CUDAToolkit_ROOT = pkgs.lib.optionalString isLinux "${pkgsUnfree.cudaPackages.cudatoolkit}";
              # rpath: the binary finds libcuda and libcudart by itself, without LD_LIBRARY_PATH,
              # because the compositor binds and the GUI start it outside this shell
              RUSTFLAGS = "-L native=/run/opengl-driver/lib -C link-arg=-Wl,-rpath,/run/opengl-driver/lib -C link-arg=-Wl,-rpath,${pkgsUnfree.cudaPackages.cudatoolkit}/lib";
              shellHook =
                base.shellHook
                + pkgs.lib.optionalString isLinux ''
                  export LD_LIBRARY_PATH="/run/opengl-driver/lib:${pkgsUnfree.cudaPackages.cudatoolkit}/lib:$LD_LIBRARY_PATH"
                  export LIBRARY_PATH="/run/opengl-driver/lib:$LIBRARY_PATH"
                '';
            }
          );

          # The build of whisper.cpp with Vulkan (cargo build --features vulkan).
          # The ready bindings of whisper-rs-sys have no ggml_backend_vk_* functions, so here
          # the bindings are generated: the variable from base is removed, and bindgenHook gives
          # libclang
          vulkan = pkgs.mkShell (
            builtins.removeAttrs base [ "WHISPER_DONT_GENERATE_BINDINGS" ]
            // {
              packages =
                base.packages
                ++ (with pkgs; [ rustPlatform.bindgenHook ])
                ++ pkgs.lib.optionals isLinux (
                  with pkgs;
                  [
                    vulkan-loader
                    vulkan-headers
                    shaderc
                  ]
                );
            }
          );
        }
      );

      # Without this output `nix fmt` does nothing here, and there would be nothing
      # to check the formatting with. nixfmt-tree walks the tree, not the files that were
      # passed to it, so one command covers future .nix files too
      formatter = forAll (pkgs: pkgs.nixfmt-tree);

      # homeModules is the name the flake schema knows; homeManagerModules is what most
      # consumers still write, so both point at the same module
      homeModules.default = import ./nix/module.nix { inherit self; };
      homeManagerModules.default = self.homeModules.default;

      checks = forAll (pkgs: {
        # Enabling the module has to be enough: the package, and under a Lua Hyprland the
        # include line and the activation step that writes the binds
        module-wiring =
          let
            wiring = import ./nix/module-test.nix {
              inherit (pkgs) lib;
              inherit pkgs;
              module = self.homeModules.default;
            };
          in
          pkgs.runCommand "module-wiring"
            {
              nativeBuildInputs = with pkgs; [ jq ];
              dump = builtins.toJSON wiring;
              passAsFile = [ "dump" ];
            }
            ''
              want() { jq -e "$1" "$dumpPath" >/dev/null || { echo "module wiring: $2"; exit 1; }; }

              want '.lua.packages | any(test("molva"))' "no package installed"
              want '(.lua.extraConfig | test("pcall\\(dofile, os.getenv\\(\"HOME\"\\) \\.\\. \"/.config/hypr/molvai.lua\"\\)")) and .lua.activation == ""' "no include line or an activation command"
              want '.lua.binds | endswith("molvai-hyprland.lua")' "no generated Hyprland bind file"
              want '.hyprlang.extraConfig == "" and .hyprlang.activation == "" and .hyprlang.binds == ""' "a Lua line reaches a hyprlang config"
              want '.optedOut.extraConfig == "" and .optedOut.activation == "" and .optedOut.binds == ""' "hyprland.enable = false is ignored"
              want '.optedOut.packages | any(test("molva"))' "opting out of Hyprland drops the package"
              want '.lua.daemon | endswith("/bin/molva daemon")' "no user service runs the daemon"
              want '.noDaemon.daemon == ""' "daemon.enable = false is ignored"
              want '.lua.guiAutostart | test("Exec=molva-gui")' "no GUI autostart entry"
              want '.noDaemon.guiAutostart == ""' "daemon.enable = false still starts the GUI"
              want '.noGuiAutostart.guiAutostart == ""' "guiAutostart.enable = false is ignored"
              want '.customGuiCommand.guiAutostart | test("Exec=/run/current-system/sw/bin/molva-gui")' "guiAutostart.command is ignored"
              want '.lua.model == "gigaam-v3-e2e-rnnt-q4_k_m"' "the default model is not the Sber one"
              want '.lua.models | index("gigaam-v3-e2e-rnnt-q4_k_m")' "the default model is not fetched"
              want '.lua.configSource | test("molvai-config.toml$")' "no generated settings file"
              want '.whisper.model == "small" and .whisper.settings.stt.chunk_pause_ms == 400' "settings do not merge"
              want '.off.packages == [] and .off.extraConfig == "" and .off.daemon == "" and .off.binds == ""' "the module acts while disabled"
              touch $out
            '';
      });
    };
}
