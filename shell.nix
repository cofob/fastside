{ channel ? "stable", profile ? "default", pkgs ? import <nixpkgs> }:
let
  pkgs' = pkgs.extend (import (builtins.fetchTarball {
    url =
      "https://github.com/oxalica/rust-overlay/archive/07601339b15fa6810541c0e7dc2f3664d92a7ad0.tar.gz";
    sha256 = "01x6kk0nln52w0x6lq58n767ngfr8df4ci5d0bd9xsky1fydqxgp";
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
