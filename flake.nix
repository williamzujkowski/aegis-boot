{
  description = "Aegis-Boot - Reproducible build environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            gcc
            pkg-config
            openssl
            python311
            python311Packages.pip
            nasm
            util-linux
            iasl
            git
          ];

          RUST_VERSION = "1.85.0";

          shellHook = ''
            echo "Aegis-Boot Build Environment"
            echo "================================"
            echo "Rust: $(rustc --version)"
            echo "Nixpkgs: ${nixpkgs.lib.version}"
          '';
        };
      }
    );
}
