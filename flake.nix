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
  };

  outputs = {
    self,
    nixpkgs,
    fenix,
    crane,
    solidity-ibc-eureka,
    sp1,
    ...
  }: let
    systems = ["x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin"];
    forAllSystems = f:
      nixpkgs.lib.genAttrs systems (system: let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [sp1.overlays.default];
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
      cargoVendorDir = craneLib.vendorCargoDeps {
        inherit src;
        outputHashes = {
          "git+https://github.com/srdtrk/ibc-proto-rs?rev=3613891e18478811216cce02dc867b7c6ff8811b#3613891e18478811216cce02dc867b7c6ff8811b" = "sha256-tzo5lOTVAAxNHXtP7+vZVsi41BvkYE8JrcBn54CIDaQ=";
        };
        overrideVendorCargoPackage = p: drv:
          if p.name == "sp1-core-machine"
          then
            drv.overrideAttrs (_old: {
              postPatch = ''
                rm -f Cargo.lock
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

      toolchain = fenixPkgs.stable.withComponents [
        "cargo"
        "clippy"
        "rustc"
        "rustfmt"
        "rust-src"
        "rust-analyzer"
      ];
    in {
      packages.default = craneLib.buildPackage (commonArgs // {inherit cargoArtifacts meta;});

      checks = {
        clippy = craneLib.cargoClippy (commonArgs
          // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "-- -D warnings";
          });
        nextest = craneLib.cargoNextest (commonArgs
          // {
            inherit cargoArtifacts;
            nativeBuildInputs = commonArgs.nativeBuildInputs ++ [pkgs.cargo-nextest];
          });
        fmt = craneLib.cargoFmt {inherit src;};
        deny = craneLib.cargoDeny (commonArgs // {inherit src;});
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
            pkgs.foundry
            pkgs.bun
          ]
          ++ (with pkgs.sp1."v5.2.4"; [
            cargo-prove
            sp1-rust-toolchain
          ])
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
