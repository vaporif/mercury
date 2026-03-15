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
  };

  outputs = {
    self,
    nixpkgs,
    fenix,
    crane,
    solidity-ibc-eureka,
    ...
  }: let
    systems = ["x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin"];
    forAllSystems = f:
      nixpkgs.lib.genAttrs systems (system:
        f {
          pkgs = nixpkgs.legacyPackages.${system};
          fenixPkgs = fenix.packages.${system};
          craneLib =
            (crane.mkLib nixpkgs.legacyPackages.${system}).overrideToolchain
            fenix.packages.${system}.stable.toolchain;
        });
  in {
    formatter = nixpkgs.lib.genAttrs systems (system: nixpkgs.legacyPackages.${system}.alejandra);

    overlays.default = final: _prev: {
      mercury-relayer = self.packages.${final.stdenv.hostPlatform.system}.default;
    };

    packages = forAllSystems ({
      pkgs,
      craneLib,
      ...
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
      # Vendor deps with a workaround for upstream solidity-ibc-eureka having
      # a relative readme path that doesn't exist when crane extracts the git dep.
      cargoVendorDir = craneLib.vendorCargoDeps {
        inherit src;
        outputHashes = {
          "git+https://github.com/srdtrk/ibc-proto-rs?rev=3613891e18478811216cce02dc867b7c6ff8811b#3613891e18478811216cce02dc867b7c6ff8811b" = "sha256-tzo5lOTVAAxNHXtP7+vZVsi41BvkYE8JrcBn54CIDaQ=";
        };
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
    in {
      default = craneLib.buildPackage (commonArgs // {inherit cargoArtifacts meta;});
    });

    devShells = forAllSystems ({
      pkgs,
      fenixPkgs,
      ...
    }: let
      toolchain = fenixPkgs.stable.withComponents [
        "cargo"
        "clippy"
        "rustc"
        "rustfmt"
        "rust-src"
        "rust-analyzer"
      ];
    in {
      default = pkgs.mkShell {
        packages =
          [
            toolchain
            pkgs.just
            pkgs.taplo
            pkgs.typos
            pkgs.actionlint
            pkgs.cargo-nextest
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.apple-sdk_15
          ];

        env = {
          RUST_BACKTRACE = "1";
          RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
        };
      };
    });
  };
}
