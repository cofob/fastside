{
  description = "A smart redirecting gateway for various frontend services";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    flake-utils.url = "github:numtide/flake-utils";

    naersk.url = "github:nix-community/naersk";
    naersk.inputs.nixpkgs.follows = "nixpkgs";

    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      naersk,
      rust-overlay,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        naersk' = pkgs.callPackage naersk {
          cargo = rustToolchain;
          rustc = rustToolchain;
        };

        # This needed to remove dependency on services.json file and avoid useless rebuilds
        constructed-source = pkgs.runCommand "constructed-source" { } ''
          mkdir -p $out
          cp -r ${./fastside} $out/fastside
          cp -r ${./fastside-actualizer} $out/fastside-actualizer
          cp -r ${./fastside-cloudflare} $out/fastside-cloudflare
          cp -r ${./fastside-shared} $out/fastside-shared
          cp ${./Cargo.toml} $out/Cargo.toml
          cp ${./Cargo.lock} $out/Cargo.lock
          cp ${./rust-toolchain.toml} $out/rust-toolchain.toml
        '';

        fastside = naersk'.buildPackage {
          name = "fastside";
          version = "0.2.0";
          src = constructed-source;
          nativeBuildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.mold ];
          NIX_CFLAGS_LINK = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux " -fuse-ld=mold";
        };

        fastside-baked-services = pkgs.writeShellScriptBin "fastside-baked-services" ''
          export FS__SERVICES=${./services.json}
          exec ${fastside}/bin/fastside "$@"
        '';

        fastside-docker = pkgs.dockerTools.buildLayeredImage {
          name = "fastside";
          tag = "latest";
          contents = [
            fastside
            pkgs.cacert
          ];
          config = {
            Cmd = [
              "/bin/fastside"
              "serve"
              "-l"
              "0.0.0.0:8080"
            ];
            Env = [ "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt" ];
          };
        };

        fastside-docker-baked-services = pkgs.dockerTools.buildLayeredImage {
          name = "fastside";
          tag = "latest";
          contents = [
            fastside
            fastside-baked-services
            pkgs.cacert
          ];
          config = {
            Cmd = [
              "/bin/fastside-baked-services"
              "serve"
              "-l"
              "0.0.0.0:8080"
            ];
            Env = [ "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt" ];
          };
        };

        services = pkgs.runCommand "generate-services" { } ''
          cat '${./services.json}' > $out
        '';
      in
      rec {
        formatter = pkgs.nixfmt;

        packages = {
          default = fastside;
          fastside = fastside;
          fastside-baked-services = fastside-baked-services;
          fastside-docker = fastside-docker;
          fastside-docker-baked-services = fastside-docker-baked-services;
          services = services;
        };

        apps = rec {
          default = fastside;
          fastside = {
            type = "app";
            program = "${packages.fastside}/bin/fastside";
          };
          fastside-backed-services = {
            type = "app";
            program = "${packages.fastside-baked-services}/bin/fastside-baked-services";
          };
          actualizer = {
            type = "app";
            program = "${packages.fastside}/bin/fastside-actualizer";
          };
          fastside-actualizer = actualizer;
        };

        devShells.default = import ./shell.nix { inherit pkgs rustToolchain; };
      }
    );
}
