{ inputs, cell }: let
  inherit (inputs) fenix;
  targets = fenix.packages.targets;
  dirtyRustPkgs = fenix.packages.complete;
  rustPkgs = builtins.removeAttrs dirtyRustPkgs ["withComponents" "name" "type"];
in
  # add rust-analyzer from nightly, if not present
  if rustPkgs ? rust-analyzer
  then
    rustPkgs // {
      toolchain = fenix.packages.combine [
        (builtins.attrValues rustPkgs)
      ];
    }
  else
    rustPkgs // {
      inherit (fenix.packages) rust-analyzer;
      toolchain = fenix.packages.combine [
        (builtins.attrValues rustPkgs)
        fenix.packages.rust-analyzer
      ];
    }
