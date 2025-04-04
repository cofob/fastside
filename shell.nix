{ channel ? "stable", profile ? "default", pkgs ? import <nixpkgs> }:
let
  pkgs' = pkgs.extend (import (builtins.fetchTarball {
    url =
      "https://github.com/oxalica/rust-overlay/archive/af76221b285a999ab7d9d77fce8ba1db028f9801.tar.gz";
    sha256 = "03zc2w66zz8dkrxpy39lrh3gqand1ypmnhcakmhibs9ndyi4v3x0";
  }));
in pkgs'.mkShell {
  nativeBuildInputs = with pkgs'; [
    nixfmt-classic
    # Rust
    ((if channel == "nightly" then
      rust-bin.selectLatestNightlyWith (toolchain: toolchain.${profile})
    else
      rust-bin.${channel}.latest.${profile}).override {
      targets = [ "wasm32-unknown-unknown" ];
    })
    wasm-pack
    wasm-bindgen-cli
  ];
}
