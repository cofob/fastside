{ channel ? "stable", profile ? "default", pkgs ? import <nixpkgs> }:
let
  pkgs' = pkgs.extend (import (builtins.fetchTarball {
    url =
      "https://github.com/oxalica/rust-overlay/archive/9803f6e04ca37a2c072783e8297d2080f8d0e739.tar.gz";
    sha256 = "1b566msx04y4s0hvwsza9gcv4djmni4fa6ik7q2m33b6x4vrb92w";
  }));
in pkgs'.mkShell {
  nativeBuildInputs = with pkgs'; [
    nixfmt-classic
    # Rust
    (if channel == "nightly" then
      rust-bin.selectLatestNightlyWith (toolchain: toolchain.${profile})
    else
      rust-bin.${channel}.latest.${profile})
  ];
}
