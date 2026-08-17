{ pkgs, rustToolchain }:
pkgs.mkShell {
  packages = with pkgs; [
    binaryen
    nixfmt
    nodejs_latest
    rustToolchain
  ];
}
