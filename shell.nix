{ pkgs, rustToolchain }:
pkgs.mkShell {
  CC_wasm32_unknown_unknown = "${pkgs.llvmPackages.clang-unwrapped}/bin/clang";
  packages = with pkgs; [
    binaryen
    nixfmt
    nodejs_latest
    rustToolchain
  ];
}
