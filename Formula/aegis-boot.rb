# Homebrew formula for aegis-boot.
#
# Install:
#   brew tap aegis-boot/aegis-boot https://github.com/aegis-boot/aegis-boot
#   brew install aegis-boot
#
# Cross-platform support: Linux x86_64 + macOS Apple Silicon (arm64)
# are shipped as of v0.16.0 (#365 Phase A1 + A3). macOS Intel, Windows,
# and Linux aarch64 remain open under #365 / #367.
class AegisBoot < Formula
  desc "Signed UEFI Secure Boot rescue environment for booting any ISO from USB"
  homepage "https://github.com/aegis-boot/aegis-boot"
  version "0.16.0"
  license any_of: ["Apache-2.0", "MIT"]

  # Runtime dependencies the operator CLI shells out to. Listed
  # explicitly so brew installs them; aegis-boot doctor will also
  # verify them at runtime. Must precede `on_linux` per brew audit
  # ComponentsOrder rule.
  depends_on "curl"
  depends_on "gnupg"
  depends_on "gptfdisk" # provides sgdisk

  on_macos do
    on_arm do
      # macOS arm64 support starts at v0.16.0 (per #365 Phase A1, which
      # added the release.yml job that builds the aarch64-apple-darwin
      # binary). Until v0.16.0 ships, this block's URL 404s and `brew
      # install` fails clean on the download step — no silent-wrong-binary
      # risk. The sha256 value is 64 zeros so it passes `brew audit` as
      # syntactically valid hex; bump-brew-formula replaces it with the
      # real sha when v0.16.0 is tagged.
      url "https://github.com/aegis-boot/aegis-boot/releases/download/v0.16.0/aegis-boot-aarch64-apple-darwin"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  on_linux do
    on_intel do
      # v0.15.0 Linux binary — real URL + sha, `brew install` works today.
      url "https://github.com/aegis-boot/aegis-boot/releases/download/v0.16.0/aegis-boot-x86_64-linux"
      sha256 "e8f22f6d87bdab539cbbcbd7e7c4f75770b5f49c9e25d6624acb6bf20e042a03"
    end
  end

  def install
    if OS.linux? && Hardware::CPU.intel?
      bin.install "aegis-boot-x86_64-linux" => "aegis-boot"
    elsif OS.mac? && Hardware::CPU.arm?
      bin.install "aegis-boot-aarch64-apple-darwin" => "aegis-boot"
    else
      odie <<~EOS
        aegis-boot binaries are currently published for:
          - Linux x86_64
          - macOS Apple Silicon (arm64)

        Support for macOS Intel, Windows, and Linux aarch64 is tracked in:
          https://github.com/aegis-boot/aegis-boot/issues/365
          https://github.com/aegis-boot/aegis-boot/issues/367

        If you're on one of the unsupported arches, build from source:
          git clone https://github.com/aegis-boot/aegis-boot
          cd aegis-boot
          cargo install --path crates/aegis-cli
      EOS
    end

    # GitHub release downloads come in without the exec bit; bin.install
    # preserves the source mode. Mode 0555 matches what brew would land
    # with post-install (read+exec for everyone, no write).
    (bin/"aegis-boot").chmod 0555

    # Generate completions + man page only when the installed binary
    # supports them. `completions` + `man` subcommands shipped in
    # #207 / #211 (post-v0.13.0); probing via `--help` is more reliable
    # than semver-parse against a floating --version format.
    help_output = Utils.safe_popen_read(bin/"aegis-boot", "--help")
    if help_output.include?("completions")
      # `shells:` constrains to the two we support — `completions fish`
      # would exit 2 otherwise and fail the install.
      generate_completions_from_executable(
        bin/"aegis-boot", "completions", shells: [:bash, :zsh]
      )
    end

    if help_output.include?("aegis-boot man")
      # Emit the man page from the same binary (self-contained via
      # include_str! in crates/aegis-cli/src/man.rs). Homebrew has no
      # built-in `generate_man_from_executable`, so shell-out and install.
      (buildpath/"aegis-boot.1").write(Utils.safe_popen_read(bin/"aegis-boot", "man"))
      man1.install "aegis-boot.1"
    end
  end

  def caveats
    binary_name = if OS.mac? && Hardware::CPU.arm?
      "aegis-boot-aarch64-apple-darwin"
    else
      "aegis-boot-x86_64-linux"
    end
    <<~EOS
      aegis-boot is a USB-imaging tool for Linux + macOS operator workstations.

      Quick start:
        aegis-boot doctor              # check host + stick health
        aegis-boot recommend           # browse the curated ISO catalog
        aegis-boot fetch ubuntu-24.04-live-server
        sudo aegis-boot flash          # write a stick (auto-detect)
        aegis-boot add ubuntu-24.04.2-live-server-amd64.iso

      Docs: https://github.com/aegis-boot/aegis-boot/blob/main/docs/INSTALL.md

      All release artifacts are Sigstore-cosign-signed. To verify the
      binary you just installed:
        cosign verify-blob \\
          --certificate-identity-regexp '^https://github\\.com/aegis-boot/aegis-boot/.+@refs/tags/v.+$' \\
          --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \\
          --signature #{binary_name}.sig \\
          --certificate #{binary_name}.pem \\
          #{binary_name}
      (download .sig + .pem from the same release)
    EOS
  end

  test do
    # Sanity: binary runs and reports the expected version.
    assert_match "aegis-boot v#{version}", shell_output("#{bin}/aegis-boot --version")
    # Help renders without panicking.
    assert_match "Signed boot. Any ISO. Your keys.",
      shell_output("#{bin}/aegis-boot --help")
    # `aegis-boot flash` without args + without removable drives
    # exits non-zero with a clear message — exercise that without
    # actually invoking dd. (Wrap shell_output with explicit exit
    # status check so brew doesn't fail on the expected non-zero.)
    output = shell_output("#{bin}/aegis-boot flash 2>&1", 1)
    assert_match(/no removable USB drives detected|not detected as one/i, output)
    # Completion files and man page — only assert when the installed
    # binary is new enough to generate them (see `def install` block
    # for the same gate). v0.13.0 predates the completions/man
    # subcommands; v0.14.0+ ships them. Once the Formula pins
    # v0.14.0 the conditional goes away.
    help_output = shell_output("#{bin}/aegis-boot --help")
    if help_output.include?("completions")
      assert_path_exists bash_completion/"aegis-boot"
      assert_path_exists zsh_completion/"_aegis-boot"
    end
    if help_output.include?("aegis-boot man")
      assert_path_exists man1/"aegis-boot.1"
      assert_match(/^\.TH AEGIS-BOOT 1/, (man1/"aegis-boot.1").read)
    end
  end
end
