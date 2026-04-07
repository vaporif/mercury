{
  lib,
  stdenv,
  fetchFromGitHub,
  rustPlatform,
  pkg-config,
  openssl,
  zlib,
  protobuf,
  perl,
  hidapi,
  udev,
  llvmPackages,
  solanaPkgs ? [
    "solana"
    "solana-faucet"
    "solana-keygen"
    "solana-test-validator"
    "solana-genesis"
  ],
}: let
  inherit (lib) optionals;
  inherit (stdenv) hostPlatform isLinux;

  version = "3.1.6";
in
  rustPlatform.buildRustPackage {
    pname = "agave";
    inherit version;

    src = fetchFromGitHub {
      owner = "anza-xyz";
      repo = "agave";
      rev = "v${version}";
      hash = "sha256-pIvShCRy1OQcFwSkXZ/lLF+2LoAd2wyAQfyyUtj9La0=";
      fetchSubmodules = true;
    };

    cargoHash = "sha256-eendPKd1oZmVqWAGWxm+AayLDm5w9J6/gSEPUXJZj88=";

    cargoBuildFlags = map (n: "--bin=${n}") solanaPkgs;

    RUSTFLAGS = "--cap-lints warn";

    nativeBuildInputs = [
      pkg-config
      protobuf
      perl
      llvmPackages.clang
    ];

    buildInputs =
      [
        openssl
        zlib
        llvmPackages.libclang.lib
      ]
      ++ optionals isLinux [
        hidapi
        udev
      ];

    LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";

    BINDGEN_EXTRA_CLANG_ARGS = toString (
      [
        "-isystem ${llvmPackages.libclang.lib}/lib/clang/${lib.getVersion llvmPackages.clang}/include"
      ]
      ++ optionals isLinux [
        "-isystem ${stdenv.cc.libc.dev}/include"
      ]
      ++ optionals hostPlatform.isDarwin [
        "-isystem ${stdenv.cc.libc}/include"
      ]
    );

    postPatch = ''
      substituteInPlace scripts/cargo-install-all.sh \
        --replace-fail './fetch-perf-libs.sh' 'echo "Skipping fetch-perf-libs in Nix build"' \
        --replace-fail '"$cargo" $maybeRustVersion install' 'echo "Skipping cargo install"'
    '';

    doCheck = false;

    meta = with lib; {
      description = "Agave Solana validator and CLI tools";
      homepage = "https://github.com/anza-xyz/agave";
      license = licenses.asl20;
      platforms = platforms.unix;
    };
  }
