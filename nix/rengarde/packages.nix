{ inputs, cell }: let
  inherit (inputs) std self cells nixpkgs;

  crane = inputs.crane.lib.overrideToolchain cells.core.rust.toolchain;

  basePkg = {
    #src = craneLib.cleanCargoSource (craneLib.path ./.);
    src = std.incl self [
      "${self}/crates"
      "${self}/Cargo.lock"
      "${self}/Cargo.toml"
    ];
    #strictDeps = true;

    # TODO: uncomment this...
    # RENGARDE_OFFICIAL_BUILD = "true";

    nativeBuildInputs = with nixpkgs; [
      pkg-config
      cmake
    ] ++ lib.optionals stdenv.buildPlatform.isDarwin [
      libiconv
    ];

    buildInputs = with nixpkgs; [
      openssl_3_2
      zlib-ng
    ];

    OPENSSL_NO_VENDOR = 1;
  };
in {
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
}
