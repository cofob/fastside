{ channel ? "stable", profile ? "default", pkgs ? import <nixpkgs> }:
let
  pkgs' = pkgs.extend (import (builtins.fetchTarball {
    url =
      "https://github.com/oxalica/rust-overlay/archive/87b6cffc276795b46ef544d7ed8d7fed6ad9c8e4.tar.gz";
    sha256 = "01gf3m4a0ljzkxf65lkcvr5kwcjr3mbpjbpppf0djk82mm98qbh4";
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
