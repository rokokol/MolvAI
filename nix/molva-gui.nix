{
  lib,
  stdenv,
  rustPlatform,
  fetchNpmDeps,
  npmDepsHash,
  cargo-tauri,
  cmake,
  nodejs,
  npmHooks,
  pkg-config,
  wrapGAppsHook4,
  glib-networking,
  gsettings-desktop-schemas,
  gtk3,
  webkitgtk_4_1,
  libsoup_3,
  openssl,
  alsa-lib,
  libxkbcommon,
  wayland,
  dbus,
  librsvg,
  libayatana-appindicator,
  source,
  version,
}:

rustPlatform.buildRustPackage (finalAttrs: {
  pname = "molva-gui";
  inherit version;

  src = source;
  cargoLock.lockFile = "${source}/Cargo.lock";
  buildAndTestSubdir = "crates/molva-gui";

  # The Tauri beforeBuildCommand runs `npm run build` in this directory
  npmRoot = "crates/molva-gui/ui";
  npmDeps = fetchNpmDeps {
    src = "${source}/crates/molva-gui/ui";
    hash = npmDepsHash;
  };

  nativeBuildInputs = [
    cargo-tauri.hook
    cmake
    nodejs
    npmHooks.npmConfigHook
    pkg-config
    wrapGAppsHook4
  ];

  buildInputs = [
    glib-networking
    gsettings-desktop-schemas
    gtk3
    webkitgtk_4_1
    libsoup_3
    openssl
    alsa-lib
    libxkbcommon
    wayland
    dbus
    librsvg
    libayatana-appindicator
  ];

  cargoBuildFeatures = [ "custom-protocol" ];
  WHISPER_DONT_GENERATE_BINDINGS = "1";
  GGML_NATIVE = "OFF";
  doCheck = false;

  meta = {
    description = "Desktop window and tray for MolvAI, system-wide voice input";
    homepage = "https://github.com/rokokol/MolvAI";
    license = lib.licenses.mit;
    mainProgram = "molva-gui";
    platforms = lib.platforms.linux;
  };
})
