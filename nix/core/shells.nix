{ inputs, cell }: let
  inherit (inputs.std) std lib;
  inherit (inputs) nixpkgs;
  inherit (inputs.cells) rengarde;

  l = nixpkgs.lib // builtins;

  dev = lib.dev.mkShell {
    imports = [
      "${inputs.std.inputs.devshell}/extra/language/rust.nix"
    ];

    language.rust = {
      packageSet = cell.rust;
      enableDefaultToolchain = true;
      tools = ["toolchain"];
    };

    packages = with nixpkgs; [
      pkg-config
      cmake
      zlib-ng
      wireguard-tools
      wireguard-vanity-address
    ];

    devshell.startup.link-cargo-home = {
      deps = [];
      text = ''
        # ensure CARGO_HOME is populated
        mkdir -p "$PRJ_DATA_DIR/cargo"
        ln -snf -t "$PRJ_DATA_DIR/cargo" $(ls -d ${cell.rust.toolchain}/*)
      '';
    };

    env = [
      {
        # ensures subcommands are picked up from the right place
        # but also needs to be writable; see link-cargo-home above
        name = "CARGO_HOME";
        eval = "$PRJ_DATA_DIR/cargo";
      }
      {
        # ensure we know where rustup_home will be
        name = "RUSTUP_HOME";
        eval = "$PRJ_DATA_DIR/rustup";
      }
      {
        name = "RUST_SRC_PATH";
        # accessing via toolchain doesn't fail if it's not there
        # and rust-analyzer is graceful if it's not set correctly:
        # https://github.com/rust-lang/rust-analyzer/blob/7f1234492e3164f9688027278df7e915bc1d919c/crates/project-model/src/sysroot.rs#L196-L211
        value = "${cell.rust.toolchain}/lib/rustlib/src/rust/library";
      }
      {
        name = "PKG_CONFIG_PATH";
        value = l.makeSearchPath "lib/pkgconfig" (
          rengarde.packages.rengarde-client.buildInputs
          ++
          rengarde.packages.rengarde-server.buildInputs
          ++
          [
            nixpkgs.openssl_3_2.dev
            nixpkgs.zlib-ng
          ]
        );
      }
      {
        name = "OPENSSL_NO_VENDOR";
        value = "1";
      }
    ];

    commands = let
      rustCmds =
        l.map (name: {
          inherit name;
          package = cell.rust.toolchain;
          category = "rust";
          help = nixpkgs.${name}.meta.description;
        }) [
          "cargo"
          "rustc"
          "rustfmt"
          "rust-analyzer"
        ];
    in
      [
        {
        package = std.cli.default;
        category = "std";
        }
        {
          package = nixpkgs.treefmt;
          category = "tools";
        }
        {
          package = nixpkgs.alejandra;
          category = "tools";
        }
      ]
      ++ rustCmds;
  };
in {
  inherit dev;
  default = dev;
}
