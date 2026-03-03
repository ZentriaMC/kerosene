{
  description = "kerosene";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils, ... }:
    let
      supportedSystems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
    in
    flake-utils.lib.eachSystem supportedSystems (system:
      let
        pkgs = import nixpkgs {
          inherit system;
        };
      in
      {
        devShell = pkgs.mkShell {
          packages = [
            pkgs.butane
            pkgs.jq
            pkgs.python3
            pkgs.qemu
          ];

          BUTANE = "${pkgs.butane}/bin/butane";
          QEMU_EFI_FW =
            if pkgs.stdenv.isx86_64 then "${pkgs.qemu}/share/qemu/edk2-x86_64-code.fd"
            else if pkgs.stdenv.isAarch64 then "${pkgs.qemu}/share/qemu/edk2-aarch64-code.fd"
            else "UNSUPPORTED";
        };
      });
}
