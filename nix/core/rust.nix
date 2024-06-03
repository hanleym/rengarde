{ inputs, cell }: let
  inherit (inputs) fenix;
  # you may change "default" to any of "[minimal|default|complete|latest]" for variants
  # see upstream fenix documentation for details
  # rustPkgs = fenix.packages.combine [
  #   fenix.packages.complete
  #   fenix.targets.aarch64-unknown-linux-gnu.latest.rust-std
  # ];
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
        #targets.aarch64-unknown-linux-gnu.latest.rust-std
      ];
    }
  else
    rustPkgs // {
      inherit (fenix.packages) rust-analyzer;
      toolchain = fenix.packages.combine [
        (builtins.attrValues rustPkgs)
        #targets.aarch64-unknown-linux-gnu.latest.rust-std
        fenix.packages.rust-analyzer
      ];
    }
