{
  description = "A smart redirecting gateway for various frontend services";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    flake-utils.url = "github:numtide/flake-utils";

    naersk.url = "github:nix-community/naersk";
    naersk.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils, naersk, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        naersk' = pkgs.callPackage naersk { };
        pkgsStatic = pkgs.pkgsStatic;
        naersk'static = pkgsStatic.callPackage naersk { };

        # This needed to remove dependency on services.json file and avoid useless rebuilds
        constructed-source = pkgs.runCommand "constructed-source" { } ''
          mkdir -p $out
          cp -r ${./fastside} $out/fastside
          cp -r ${./fastside-actualizer} $out/fastside-actualizer
          cp -r ${./fastside-shared} $out/fastside-shared
          cp ${./Cargo.toml} $out/Cargo.toml
          cp ${./Cargo.lock} $out/Cargo.lock
        '';

        fastside-attrs = {
          name = "fastside-0.2.0";
          src = constructed-source;
          nativeBuildInputs = with pkgs; [ mold ];
          NIX_CFLAGS_LINK = " -fuse-ld=mold";
        };

        fastside = naersk'.buildPackage fastside-attrs;

        fastside-static = naersk'static.buildPackage fastside-attrs;

        fastside-baked-services = pkgs.writeShellScriptBin "fastside-baked-services" ''
          export FS__SERVICES_PATH=${./services.json}
          ${fastside}/bin/fastside $@
        '';

        fastside-baked-services-static = pkgs.writeShellScriptBin "fastside-baked-services" ''
          export FS__SERVICES_PATH=${./services.json}
          ${fastside-static}/bin/fastside $@
        '';

        fastside-docker = pkgs.dockerTools.buildLayeredImage {
          name = "fastside";
          tag = "latest";
          contents = [ fastside-static ];
          config = { Cmd = [ "/bin/fastside" "serve" "-l" "0.0.0.0:8080" ]; };
        };

        fastside-docker-baked-services = pkgs.dockerTools.buildLayeredImage {
          name = "fastside";
          tag = "latest";
          contents = [ fastside-static fastside-baked-services-static ];
          config = { Cmd = [ "/bin/fastside-baked-services" "serve" "-l" "0.0.0.0:8080" ]; };
        };

        services = pkgs.runCommand "generate-services" { } ''
          cat '${./services.json}' > $out
        '';
      in rec {
        packages = {
          default = fastside;
          fastside = fastside;
          fastside-static = fastside-static;
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

        devShells.default = import ./shell.nix { inherit pkgs; };
      });
}
