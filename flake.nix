{
  description = "aegis-boot — signed UEFI Secure Boot rescue environment";

  inputs = {
    # Pinned to a stable channel with rust ≥1.88 (our MSRV). nixos-25.11
    # currently ships rustc 1.91.x — well above the floor. CI builds
    # use rustc 1.95 via rust-toolchain.toml; this flake honors
    # whatever rustc nixos-25.11 happens to ship as long as it ≥ MSRV.
    # Avoids nixos-unstable churn (python/sphinx breakage observed
    # mid-PR #406).
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        # Runtime tools the `aegis-boot` CLI shells out to on Linux.
        # Baked into the binary's PATH via `makeWrapper` so NixOS users
        # don't have to install them globally. `aegis-boot doctor`
        # enumerates the same list — keep in sync with the doctor
        # checks in crates/aegis-cli/src/doctor.rs.
        runtimeDeps = with pkgs; [
          gptfdisk      # sgdisk — GPT partitioning
          dosfstools    # mkfs.fat — ESP
          exfatprogs    # mkfs.exfat — AEGIS_ISOS
          mtools        # mcopy, mmd — staging into FAT
          curl          # aegis-boot fetch
          gnupg         # SHA256SUMS.sig verification
          coreutils     # sha256sum
        ];

        aegis-bootctl = pkgs.rustPlatform.buildRustPackage {
          pname = "aegis-bootctl";
          version = "0.16.0";

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          # Build only the operator CLI binary from the workspace.
          # rescue-tui, initramfs bits, fuzz targets, and the
          # docgen-only bins are out of scope for a host-side install.
          cargoBuildFlags = [ "-p" "aegis-bootctl" "--bin" "aegis-boot" ];
          cargoTestFlags = [ "-p" "aegis-bootctl" ];

          nativeBuildInputs = with pkgs; [ makeWrapper pkg-config ];
          buildInputs = with pkgs; [ openssl ];

          # Ensure the binary finds its runtime tools without the user
          # having to install them separately. Matches the behavior of
          # the `doctor` check — if this wrapper changes, update
          # doctor's error message for the matching dep.
          postFixup = ''
            wrapProgram $out/bin/aegis-boot \
              --prefix PATH : ${pkgs.lib.makeBinPath runtimeDeps}
          '';

          meta = with pkgs.lib; {
            description = "Operator CLI for aegis-boot — flash, add, list, verify signed rescue sticks";
            homepage = "https://github.com/aegis-boot/aegis-boot";
            license = with licenses; [ mit asl20 ];
            mainProgram = "aegis-boot";
            platforms = platforms.linux;
          };
        };
      in
      {
        packages = {
          default = aegis-bootctl;
          aegis-bootctl = aegis-bootctl;
        };

        apps.default = {
          type = "app";
          program = "${aegis-bootctl}/bin/aegis-boot";
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            gcc
            pkg-config
            openssl
            python312
            python312Packages.pip
            nasm
            util-linux
            acpica-tools
            git
          ] ++ runtimeDeps;

          shellHook = ''
            echo "aegis-boot build environment"
            echo "================================"
            echo "Rust: $(rustc --version)"
            echo "Nixpkgs: ${nixpkgs.lib.version}"
          '';
        };
      }
    ) // {
      # NixOS module — users import this into their system flake to
      # install aegis-boot declaratively alongside its runtime deps.
      nixosModules.aegis-boot = { pkgs, lib, config, ... }:
        let
          cfg = config.programs.aegis-boot;
        in
        {
          options.programs.aegis-boot = {
            enable = lib.mkEnableOption "aegis-boot operator CLI";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.system}.aegis-bootctl;
              description = "Which aegis-bootctl derivation to install.";
            };
          };
          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];
          };
        };
    };
}
