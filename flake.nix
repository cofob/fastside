{
  description = "A smart redirecting gateway for various frontend services";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    flake-utils.url = "github:numtide/flake-utils";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };
    import-cargo.url = "github:edolstra/import-cargo";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, import-cargo, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        rustVersion = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "x86_64-unknown-linux-musl" ];
        };
        inherit (import-cargo.builders) importCargo;

        nativeBuildInputs = with pkgs; [ pkg-config rustVersion ];

        buildInputs = with pkgs; [ ];

        fastside = pkgs.stdenv.mkDerivation {
          pname = "fastside";
          version = "0.1.0";

          src = self;

          inherit buildInputs;

          nativeBuildInputs = [
            (importCargo {
              lockFile = ./Cargo.lock;
              inherit pkgs;
            }).cargoHome
          ] ++ nativeBuildInputs;

          buildPhase = ''
            cargo build --release --offline --target x86_64-unknown-linux-musl
          '';

          installPhase = ''
            install -Dm775 ./target/x86_64-unknown-linux-musl/release/fastside $out/bin/fastside
          '';
        };

        fastside-docker = pkgs.dockerTools.buildLayeredImage {
          name = "fastside";
          tag = "latest";
          contents = [ fastside ];
          config = { Cmd = [ "/bin/fastside" "serve" ]; };
        };
      in {
        packages = {
          default = fastside;
          fastside = fastside;
          fastside-docker = fastside-docker;
          services = ./services.json;
        };

        devShells.default = pkgs.mkShell {
          buildInputs = (with pkgs; [ nixfmt ]) ++ nativeBuildInputs
            ++ buildInputs;
        };
      });
}
