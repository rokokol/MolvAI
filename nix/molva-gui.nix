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
  gst_all_1,
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
    # WebKitGTK draws and decodes media through GStreamer; the plugins are a separate input, and
    # without the H.264 and AAC ones a video never starts playing
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
    gst_all_1.gst-plugins-good
    gst_all_1.gst-plugins-bad
    gst_all_1.gst-libav
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

  # The tray crate loads libayatana-appindicator3 at runtime, so no NEEDED entry puts it
  # into RUNPATH; the wrapper has to name it. The media plugins are found by their directory
  # too: WebKit asks GStreamer, and GStreamer reads this path
  preFixup = ''
    gappsWrapperArgs+=(
      --prefix LD_LIBRARY_PATH : "${libayatana-appindicator}/lib"
      --prefix GST_PLUGIN_SYSTEM_PATH_1_0 : "${lib.makeSearchPathOutput "lib" "lib/gstreamer-1.0" [
        gst_all_1.gstreamer
        gst_all_1.gst-plugins-base
        gst_all_1.gst-plugins-good
        gst_all_1.gst-plugins-bad
        gst_all_1.gst-libav
      ]}"
    )
  '';

  meta = {
    description = "Desktop window and tray for MolvAI, system-wide voice input";
    homepage = "https://github.com/rokokol/MolvAI";
    license = lib.licenses.mit;
    mainProgram = "molva-gui";
    platforms = lib.platforms.linux;
  };
})
