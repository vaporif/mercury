{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane = {
      url = "github:ipetkov/crane";
    };
    solidity-ibc-eureka = {
      url = "github:cosmos/solidity-ibc-eureka/86505ac8c69be4e955f8b7d3baafbd0fddaeefee";
      flake = false;
    };
    sp1 = {
      url = "github:vaporif/sp1-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    anchor = {
      url = "github:vaporif/anchor-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.crane.follows = "crane";
    };
    ethereum-nix = {
      url = "github:nix-community/ethereum.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    fenix,
    crane,
    solidity-ibc-eureka,
    sp1,
    anchor,
    ethereum-nix,
    ...
  }: let
    systems = ["x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin"];
    forAllSystems = f:
      nixpkgs.lib.genAttrs systems (system: let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [sp1.overlays.default anchor.overlays.default];
        };
      in
        f {
          inherit pkgs;
          fenixPkgs = fenix.packages.${system};
          craneLib =
            (crane.mkLib pkgs).overrideToolchain
            fenix.packages.${system}.stable.toolchain;
        });

    # Per-system build context shared by packages, checks, and devShells.
    perSystem = forAllSystems ({
      pkgs,
      fenixPkgs,
      craneLib,
    }: let
      # Overlay ABI JSON files from the solidity-ibc-eureka flake input
      # since Nix flakes don't include git submodules.
      src = pkgs.stdenvNoCC.mkDerivation {
        name = "mercury-src";
        src = craneLib.cleanCargoSource ./.;
        buildInputs = [];
        installPhase = ''
          runHook preInstall
          cp -r . $out
          mkdir -p $out/external/solidity-ibc-eureka/abi
          cp ${solidity-ibc-eureka}/abi/*.json $out/external/solidity-ibc-eureka/abi/
          runHook postInstall
        '';
      };

      # Vendor deps with workarounds:
      # 1. solidity-ibc-eureka has relative readme paths that don't exist when crane extracts the git dep
      # 2. sp1-core-machine ships a stale Cargo.lock pinning cfg-if 1.0.0; its build.rs uses cbindgen
      #    which runs `cargo metadata` and picks up that lockfile, but the vendor dir has cfg-if 1.0.4
      # sp1-prover build.rs downloads vk_map.bin from S3 at build time.
      # Pre-fetch it so the sandboxed Nix build doesn't need network.
      sp1VkMap = pkgs.fetchurl {
        url = "https://sp1-circuits.s3.us-east-2.amazonaws.com/vk-map-v5.0.0";
        hash = "sha256-XnNfbkT1bp7ukeViYlJmOvzFJjKH0cWYA2ez+fkwoOg=";
      };
      cargoVendorDir = craneLib.vendorCargoDeps {
        inherit src;
        outputHashes = {
          "git+https://github.com/srdtrk/ibc-proto-rs?rev=3613891e18478811216cce02dc867b7c6ff8811b#3613891e18478811216cce02dc867b7c6ff8811b" = "sha256-tzo5lOTVAAxNHXtP7+vZVsi41BvkYE8JrcBn54CIDaQ=";
        };
        # sp1-core-machine 5.2.4's build.rs runs cbindgen which calls
        # `cargo metadata` against its own Cargo.toml. This fails in Nix's
        # sealed vendor dir because optional/dev deps aren't vendored.
        # Patch build.rs to skip the cbindgen FFI generation entirely.
        # Safe to remove once upgraded to sp1 >= 6.0.2 (no build.rs).
        overrideVendorCargoPackage = p: drv:
          if p.name == "sp1-prover"
          then
            # Provide pre-fetched vk_map.bin so build.rs doesn't need network
            drv.overrideAttrs (old: {
              postPatch =
                (old.postPatch or "")
                + ''
                  mkdir -p src
                  cp ${sp1VkMap} src/vk_map.bin
                '';
            })
          else if builtins.elem p.name ["sp1-core-machine" "sp1-recursion-core"]
          then
            # cbindgen runs `cargo metadata` in build.rs which fails in Nix's
            # sealed vendor dir (unvendored optional/dev deps). Strip dev-deps,
            # remove stale Cargo.lock. Safe to remove with sp1 >= 6.0.2.
            drv.overrideAttrs (old: {
              nativeBuildInputs = (old.nativeBuildInputs or []) ++ [pkgs.gnused pkgs.gawk];
              postPatch =
                (old.postPatch or "")
                + ''
                  rm -f Cargo.lock
                  awk '/^\[dev-dependencies/{skip=1; next} /^\[/{skip=0} !skip' Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml
                '';
            })
          else if p.name == "cbindgen"
          then
            drv.overrideAttrs (old: {
              nativeBuildInputs = (old.nativeBuildInputs or []) ++ [pkgs.gnused];
              # Copy manifest dir to writable tmpdir before calling cargo metadata
              # so cargo can write Cargo.lock. Nix store is read-only.
              postPatch =
                (old.postPatch or "")
                + ''
                  sed -i 's|cmd\.arg(manifest_path);|{ let tmp = std::env::temp_dir().join(format!("cbindgen-meta-{}", std::process::id())); let _ = std::fs::create_dir_all(\&tmp); let src = manifest_path.parent().unwrap(); for e in std::fs::read_dir(src).into_iter().flatten().flatten() { let p = e.path(); if p.is_file() { let _ = std::fs::copy(\&p, tmp.join(e.file_name())); } else if p.is_dir() { let dst = tmp.join(e.file_name()); let _ = std::os::unix::fs::symlink(\&p, \&dst); } } cmd.arg(tmp.join("Cargo.toml")); }|' src/bindgen/cargo/cargo_metadata.rs
                '';
            })
          else if p.name == "sp1-curves"
          then
            # Strip unvendored optional dep `rug` — cargo metadata verifies
            # all deps exist even when their feature isn't enabled.
            drv.overrideAttrs (old: {
              nativeBuildInputs = (old.nativeBuildInputs or []) ++ [pkgs.gnused];
              postPatch =
                (old.postPatch or "")
                + ''
                  sed -i '/^\[dependencies\.rug\]/,/^$/d; s/bigint-rug = \["rug"\]/bigint-rug = []/' Cargo.toml
                '';
            })
          else drv;
        overrideVendorGitCheckout = ps: drv:
          if pkgs.lib.any (p: pkgs.lib.hasPrefix "git+https://github.com/cosmos/solidity-ibc-eureka" p.source) ps
          then
            drv.overrideAttrs (old: {
              nativeBuildInputs = (old.nativeBuildInputs or []) ++ [pkgs.gnused];
              postPatch = ''
                find . -name Cargo.toml -exec \
                  sed -i '/^readme\s*=.*\.\.\/.*README/d' {} +
              '';
              postInstall = ''
                cp -r ${solidity-ibc-eureka}/contracts $out/
                cp -r ${solidity-ibc-eureka}/abi $out/
                find $out -name '*.rs' -exec \
                  sed -i 's|"\.\./\.\./contracts|"../contracts|g; s|"\.\./\.\./abi|"../abi|g' {} +
              '';
            })
          else drv;
      };
      commonArgs = {
        inherit src cargoVendorDir;
        pname = "mercury-relayer";
        strictDeps = true;
        nativeBuildInputs = [
          pkgs.cmake
          pkgs.pkg-config
        ];
        buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [
          pkgs.libiconv
          pkgs.apple-sdk_15
        ];
      };
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      meta = {
        description = "Mercury";
        license = pkgs.lib.licenses.mit;
        mainProgram = "mercury-relayer";
      };

      agave = pkgs.callPackage ./nix/agave.nix {
        rust-bin = anchor.inputs.rust-overlay.lib.mkRustBin {} pkgs.buildPackages;
      };

      toolchain = fenixPkgs.combine [
        (fenixPkgs.stable.withComponents [
          "cargo"
          "clippy"
          "llvm-tools-preview"
          "rustc"
          "rustfmt"
          "rust-src"
          "rust-analyzer"
        ])
        fenixPkgs.targets.wasm32-unknown-unknown.stable.rust-std
      ];
    in {
      packages.default = craneLib.buildPackage (commonArgs // {inherit cargoArtifacts meta;});

      checks = {
        fmt = craneLib.cargoFmt {inherit src;};
        typos = pkgs.runCommand "typos" {nativeBuildInputs = [pkgs.typos];} ''
          typos ${src}
          touch $out
        '';
        taplo = pkgs.runCommand "taplo" {nativeBuildInputs = [pkgs.taplo];} ''
          taplo check ${src}
          touch $out
        '';
        nix-fmt = pkgs.runCommand "nix-fmt" {nativeBuildInputs = [pkgs.alejandra];} ''
          alejandra --check ${self}/flake.nix
          touch $out
        '';
      };

      devShells.default = pkgs.mkShell {
        packages =
          [
            toolchain
            pkgs.just
            pkgs.taplo
            pkgs.typos
            pkgs.actionlint
            pkgs.cargo-nextest
            pkgs.cargo-deny
            pkgs.cargo-llvm-cov
            pkgs.protobuf
            pkgs.foundry
            pkgs.bun
            pkgs.binaryen
            ethereum-nix.packages.${pkgs.stdenv.hostPlatform.system}.kurtosis
          ]
          ++ (with pkgs.sp1."v5.2.4"; [
            cargo-prove
            sp1-rust-toolchain
          ])
          ++ (with pkgs.anchor."0.32.1"; [
            anchor-cli
            solana-rust
          ])
          ++ [agave]
          ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.apple-sdk_15
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            pkgs.podman
            (pkgs.writeShellScriptBin "docker" ''exec podman "$@"'')
          ];

        env = {
          RUST_BACKTRACE = "1";
          RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
        };
      };
    });
  in {
    formatter = nixpkgs.lib.genAttrs systems (system: nixpkgs.legacyPackages.${system}.alejandra);

    overlays.default = final: _prev: {
      mercury-relayer = self.packages.${final.stdenv.hostPlatform.system}.default;
    };

    packages = nixpkgs.lib.genAttrs systems (system: perSystem.${system}.packages);
    checks = nixpkgs.lib.genAttrs systems (system: perSystem.${system}.checks);
    devShells = nixpkgs.lib.genAttrs systems (system: perSystem.${system}.devShells);
  };
}
