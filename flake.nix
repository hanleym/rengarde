{
  inputs = {
    flake-parts.url = "github:hercules-ci/flake-parts";

    crane.url = "github:ipetkov/crane";

    fenix.url = "github:nix-community/fenix";
    nixpkgs.follows = "fenix/nixpkgs";

    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  outputs = inputs @ { self, flake-parts, ... }: flake-parts.lib.mkFlake { inherit inputs; } (top @ { config, withSystem, moduleWithSystem, ... }: {
    flake = {};

    systems = [
      "x86_64-linux"
      "aarch64-linux"
      "x86_64-darwin"
      "aarch64-darwin"
    ];
    perSystem = perSystem @ { config, system, lib, pkgs, ... }: let
      rustToolchain = inputs.fenix.packages.${system}.complete.withComponents [
        "cargo"
        "clippy"
        "rustc"
        "rust-src"
        "rustfmt"
      ];
      craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;

      commonArgs = {
        version = if lib.hasAttr "rev" self then self.rev else self.dirtyRev;
        src = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            (craneLib.fileset.commonCargoSources ./.)
          ];
        };
        strictDeps = true;
        buildInputs = [];
      };

      cargoArtifacts = craneLib.buildDepsOnly (commonArgs // {
        pname = "rengarde-deps";
      });

      rengarde-client = craneLib.buildPackage (commonArgs // {
        inherit cargoArtifacts;
        pname = "rengarde-client";
        meta.mainProgram = "rengarde";
        cargoExtraArgs = "-p client";
        doCheck = false;
      });

      rengarde-server = craneLib.buildPackage (commonArgs // {
        inherit cargoArtifacts;
        pname = "rengarde-server";
        meta.mainProgram = "rengarde";
        cargoExtraArgs = "-p server";
        doCheck = false;
      });
    in {
      devShells.default = craneLib.devShell rec {
        buildInputs = with pkgs; [];
        LD_LIBRARY_PATH = "${lib.makeLibraryPath buildInputs}";
        NIX_RUST_TOOLCHAIN = rustToolchain;
      };
      packages = {
        inherit rengarde-client rengarde-server;
      };
    };
  });

  nixConfig = {
    extra-substituters = [
      "https://nix-community.cachix.org"
    ];
    extra-trusted-public-keys = [
      "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
    ];
  };
}
