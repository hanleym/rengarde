{ inputs, cell }: let
  inherit (inputs) std self cells nixpkgs;

  crane = (inputs.crane.mkLib nixpkgs).overrideToolchain cells.core.rust.toolchain;

  basePkg = {
    #src = craneLib.cleanCargoSource (craneLib.path ./.);
    src = std.incl self [
      "${self}/crates"
      "${self}/Cargo.lock"
      "${self}/Cargo.toml"
    ];
    strictDeps = true;
    GIT_REV = self.dirtyRev or self.rev or "UNKNOWN";
  };
  rengarde-client = crane.buildPackage (basePkg // {
    meta.mainProgram = "client";
    pname = "rengarde-client";
    cargoExtraArgs = "-p client";
  });
  rengarde-server = crane.buildPackage (basePkg // {
    meta.mainProgram = "server";
    pname = "rengarde-server";
    cargoExtraArgs = "-p server";
  });
in {
  inherit rengarde-client rengarde-server;
  default = nixpkgs.symlinkJoin {
    name = "rengarde";
    paths = [
      rengarde-client
      rengarde-server
    ];
  };
}
