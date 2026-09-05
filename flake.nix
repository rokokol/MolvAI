{
  description = "MolvAI — открытый системный голосовой ввод (аналог Wispr Flow)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      # `nix run github:rokokol/MolvAI` — сборка и запуск CLI одной командой,
      # без клонирования и без установки тулчейна в систему
      packages = forAll (pkgs:
        let
          isLinux = pkgs.stdenv.hostPlatform.isLinux;
          molva = pkgs.rustPlatform.buildRustPackage {
            pname = "molva";
            # Версия читается из Cargo.toml, а не повторяется здесь: два источника
            # разъезжаются, и первым это замечает пользователь
            version = (builtins.fromTOML (builtins.readFile ./Cargo.toml))
              .workspace.package.version;
            src = self;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "--bin" "molva" ];

            nativeBuildInputs = with pkgs; [ cmake pkg-config ];
            buildInputs = with pkgs; [ openssl ]
              ++ pkgs.lib.optionals isLinux [ alsa-lib libxkbcommon ];

            # Биндинги whisper-rs лежат в крейте — libclang при сборке не нужен
            WHISPER_DONT_GENERATE_BINDINGS = "1";
            # Воспроизводимая сборка без -march=native, но с SIMD: x86-64-v3 (AVX2/FMA/F16C) —
            # иначе whisper.cpp под Nix собирается скалярно и работает в 20 раз медленнее
            GGML_NATIVE = "OFF";
          } // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isx86_64 {
            GGML_AVX = "ON";
            GGML_AVX2 = "ON";
            GGML_FMA = "ON";
            GGML_F16C = "ON";
          } // {

            meta = {
              description = "Системный голосовой ввод с обработкой на своём компьютере";
              homepage = "https://github.com/rokokol/MolvAI";
              license = pkgs.lib.licenses.mit;
              mainProgram = "molva";
            };
          };
        in
        {
          inherit molva;
          default = molva;
        });

      devShells = forAll (pkgs:
        let
          isLinux = pkgs.stdenv.hostPlatform.isLinux;

          # Тулчейн Rust и инструменты гейта
          rustTools = with pkgs; [
            cargo rustc rustfmt clippy rust-analyzer
            cargo-tauri cargo-deny cargo-about cargo-llvm-cov cargo-audit cargo-machete
            cargo-cyclonedx
            just
          ];

          # Сборка whisper.cpp и нативных зависимостей
          nativeBuild = with pkgs; [ cmake pkg-config nodejs ];

          # Библиотеки ядра
          coreLibs = with pkgs; [ openssl ]
            ++ pkgs.lib.optionals isLinux [ alsa-lib ];

          # Tauri 2 на Linux: webkit, gtk, трей
          tauriLibs = with pkgs; pkgs.lib.optionals isLinux [
            webkitgtk_4_1 libsoup_3 gtk3 glib librsvg libayatana-appindicator
            dbus wayland libxkbcommon
          ];

          # Инструменты для демо на Wayland
          waylandTools = with pkgs; pkgs.lib.optionals isLinux [ wtype wl-clipboard ];

          base = {
            packages = rustTools ++ nativeBuild ++ coreLibs ++ tauriLibs ++ waylandTools;

            # Биндинги whisper-rs уже лежат в крейте — libclang не нужен
            WHISPER_DONT_GENERATE_BINDINGS = "1";
            # Nix задаёт SOURCE_DATE_EPOCH, и ggml из-за этого выключает GGML_NATIVE: whisper.cpp
            # собирается без AVX2/FMA и работает в 20 раз медленнее. Для devShell — под свою машину.
            GGML_NATIVE = "ON";
            RUST_BACKTRACE = "1";
            # NVIDIA + Wayland: без этого окно webkit остаётся пустым
            WEBKIT_DISABLE_DMABUF_RENDERER = "1";

            shellHook = pkgs.lib.optionalString isLinux ''
              export XDG_DATA_DIRS="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}:$XDG_DATA_DIRS"
            '';
          };
        in
        {
          default = pkgs.mkShell base;

          # Сборка whisper.cpp с CUDA (cargo build --features cuda)
          cuda = pkgs.mkShell (base // {
            packages = base.packages ++ pkgs.lib.optionals isLinux [ pkgs.cudaPackages.cudatoolkit ];
            CUDA_PATH = pkgs.lib.optionalString isLinux "${pkgs.cudaPackages.cudatoolkit}";
            shellHook = base.shellHook + pkgs.lib.optionalString isLinux ''
              export LD_LIBRARY_PATH="/run/opengl-driver/lib:${pkgs.cudaPackages.cudatoolkit}/lib:$LD_LIBRARY_PATH"
            '';
          });

          # Сборка whisper.cpp с Vulkan (cargo build --features vulkan)
          vulkan = pkgs.mkShell (base // {
            packages = base.packages ++ pkgs.lib.optionals isLinux (with pkgs; [ vulkan-loader vulkan-headers shaderc ]);
          });
        });
    };
}
