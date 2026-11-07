# transcribe.cpp builds one CLI that runs GGUF models of many families. MolvAI uses it as
# the recognizer for GigaAM, which whisper.cpp cannot load. The build stays on the CPU and
# off -march=native: a Nix binary must run on another machine of the same system.
{
  lib,
  stdenv,
  cmake,
  src,
}:

stdenv.mkDerivation {
  pname = "transcribe-cpp";
  version = "0.4.1";

  inherit src;

  nativeBuildInputs = [ cmake ];

  cmakeFlags = [
    "-DTRANSCRIBE_BUILD_TESTS=OFF"
    "-DTRANSCRIBE_BUILD_EXAMPLES=ON"
    "-DTRANSCRIBE_BUILD_TOOLS=OFF"
    "-DTRANSCRIBE_USE_SYSTEM_BLAS=OFF"
    "-DGGML_NATIVE=OFF"
  ];

  installPhase = ''
    install -Dm755 bin/transcribe-cli $out/bin/transcribe-cli
  '';

  meta = {
    description = "Offline speech recognition for GGUF models";
    homepage = "https://github.com/handy-computer/transcribe.cpp";
    license = lib.licenses.mit;
    mainProgram = "transcribe-cli";
    platforms = lib.platforms.unix;
  };
}
