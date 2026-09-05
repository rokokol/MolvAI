{
  description = "MolvAI — открытый системный голосовой ввод (аналог Wispr Flow)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" "x86_64-darwin" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
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
