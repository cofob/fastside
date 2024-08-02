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

        fastside = naersk'.buildPackage {
          name = "fastside-0.2.0";
          src = pkgs.lib.cleanSourceWith {
            filter = (path: type: path != "services.json");
            src = pkgs.lib.cleanSource ./.;
          };
          nativeBuildInputs = with pkgs; [ mold ];
          NIX_CFLAGS_LINK = " -fuse-ld=mold";
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
          services = pkgs.runCommand "generate-services" { } ''
            cat '${./services.json}' > $out
          '';
        };

        devShells.default = import ./shell.nix { inherit pkgs; };
      });
}
