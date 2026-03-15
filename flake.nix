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
      # Vendor deps with workarounds:
      # 1. solidity-ibc-eureka has relative readme paths that don't exist when crane extracts the git dep
      # 2. sp1-core-machine: remove stale Cargo.lock (cbindgen runs `cargo metadata` which picks it up)
      #    and strip dev-dependencies (cbindgen's `cargo metadata` tries to resolve them but they aren't vendored)
      cargoVendorDirBase = craneLib.vendorCargoDeps {
        inherit src;
        outputHashes = {
          "git+https://github.com/srdtrk/ibc-proto-rs?rev=3613891e18478811216cce02dc867b7c6ff8811b#3613891e18478811216cce02dc867b7c6ff8811b" = "sha256-tzo5lOTVAAxNHXtP7+vZVsi41BvkYE8JrcBn54CIDaQ=";
        };
        # sp1-core-machine and sp1-recursion-core use cbindgen which calls
        # `cargo metadata --all-features`. This needs: (1) dev-deps stripped
        # (not vendored), (2) stale Cargo.lock removed, (3) build.rs patched
        # to copy crate to writable tmpdir (nix store is read-only).
        # sp1-curves has optional `rug` dep resolved by --all-features but not vendored.
        overrideVendorCargoPackage = p: drv:
          if builtins.elem p.name ["sp1-core-machine" "sp1-recursion-core"]
          then
            drv.overrideAttrs (_old: {
              nativeBuildInputs = [pkgs.gnused];
              postPatch = ''
                sed -i '/^\[dev-dependencies/,/^$/{d;}' Cargo.toml
                rm -f Cargo.lock
                sed -i '/let crate_dir = PathBuf/a\        let crate_dir = { let tmp = std::env::temp_dir().join("sp1-cbindgen-${p.name}"); fn cp(s: \&std::path::Path, d: \&std::path::Path) { let _ = std::fs::create_dir_all(d); for e in std::fs::read_dir(s).into_iter().flatten() { let e = e.unwrap(); let p = e.path(); let t = d.join(e.file_name()); if p.is_dir() { cp(\&p, \&t); } else { let _ = std::fs::copy(\&p, \&t); } } } cp(\&crate_dir, \&tmp); tmp };' build.rs
              '';
            })
          else if p.name == "sp1-curves"
          then
            drv.overrideAttrs (_old: {
              nativeBuildInputs = [pkgs.gnused];
              postPatch = ''
                sed -i '/^\[dependencies\.rug\]/,/^$/{d;}' Cargo.toml
                sed -i 's/bigint-rug = \["rug"\]/bigint-rug = []/' Cargo.toml
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
      # ibc-eureka-solidity-types uses sol!() and alloy_sol_macro with relative
      # paths (../../contracts/, ../../abi/) from its src dir. These resolve to
      # the vendor-cargo-deps root, outside the git checkout hash dir. Wrap the
      # vendor dir to inject them at the top level.
      # ibc-eureka-solidity-types uses sol!() and alloy_sol_macro with relative
      # paths (../../contracts/, ../../abi/) from its src dir. These resolve to
      # the git checkout hash dir inside vendor-cargo-deps. Inject the Solidity
      # source files there so the proc macros can find them at build time.
      cargoVendorDir = pkgs.stdenvNoCC.mkDerivation {
        name = "vendor-cargo-deps";
        src = cargoVendorDirBase;
        dontUnpack = true;
        installPhase = ''
          mkdir -p $out
          # tar -h dereferences symlinks, producing a fully materialized copy
          tar -chf - -C ${cargoVendorDirBase} . | tar -xf - -C $out
          chmod -R u+w $out
          for d in $out/*/ibc-eureka-solidity-types-*/; do
            parent=$(dirname "$d")
            cp -r ${solidity-ibc-eureka}/contracts "$parent/contracts"
            cp -r ${solidity-ibc-eureka}/abi "$parent/abi"
          done
        '';
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
            pkgs.foundry-bin
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
