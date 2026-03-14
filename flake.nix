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
  };

  outputs = {
    self,
    nixpkgs,
    fenix,
    crane,
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
      jsonFilter = path: _type: builtins.match ".*\\.json$" path != null;
      src = pkgs.lib.cleanSourceWith {
        src = craneLib.cleanCargoSource ./.;
        filter = path: type:
          (jsonFilter path type) || (craneLib.filterCargoSources path type);
      };
      commonArgs = {
        inherit src;
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
