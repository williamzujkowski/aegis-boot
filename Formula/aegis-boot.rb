# Homebrew formula for aegis-boot.
#
# Install:
#   brew tap williamzujkowski/aegis-boot https://github.com/williamzujkowski/aegis-boot
#   brew install aegis-boot
#
# Cross-platform support: Linux/x86_64 only today. macOS, Windows,
# and Linux/aarch64 builds are tracked under #123 and #137.
class AegisBoot < Formula
  desc "Signed UEFI Secure Boot rescue environment for booting any ISO from USB"
  homepage "https://github.com/williamzujkowski/aegis-boot"
  version "0.12.0"
  license any_of: ["Apache-2.0", "MIT"]

  on_linux do
    on_intel do
      url "https://github.com/williamzujkowski/aegis-boot/releases/download/v0.12.0/aegis-boot-x86_64-linux"
      sha256 "2c1b15f423823532766859ee70f929aeebce05cae5901fa080512ca7d1340953"
    end
  end

  # Runtime dependencies the operator CLI shells out to. Listed
  # explicitly so brew installs them; aegis-boot doctor will also
  # verify them at runtime.
  depends_on "curl"
  depends_on "gnupg"
  depends_on "gptfdisk" # provides sgdisk
  uses_from_macos "coreutils" # provides sha256sum (Linux always has it)

  def install
    if OS.linux? && Hardware::CPU.intel?
      bin.install "aegis-boot-x86_64-linux" => "aegis-boot"
    else
      odie <<~EOS
        aegis-boot binaries are currently published only for Linux x86_64.

        Cross-platform support is tracked in:
          https://github.com/williamzujkowski/aegis-boot/issues/123
          https://github.com/williamzujkowski/aegis-boot/issues/137

        Build from source today:
          git clone https://github.com/williamzujkowski/aegis-boot
          cd aegis-boot
          cargo install --path crates/aegis-cli
      EOS
    end
  end

  def caveats
    <<~EOS
      aegis-boot is a USB-imaging tool intended for Linux operator workstations.

      Quick start:
        aegis-boot doctor              # check host + stick health
        aegis-boot recommend           # browse the curated ISO catalog
        aegis-boot fetch ubuntu-24.04-live-server
        sudo aegis-boot flash          # write a stick (auto-detect)
        aegis-boot add ubuntu-24.04.2-live-server-amd64.iso

      Docs: https://github.com/williamzujkowski/aegis-boot/blob/main/docs/INSTALL.md

      All release artifacts are Sigstore-cosign-signed. To verify the
      binary you just installed:
        cosign verify-blob \\
          --certificate-identity-regexp '^https://github\\.com/williamzujkowski/aegis-boot/.+@refs/tags/v.+$' \\
          --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \\
          --signature aegis-boot-x86_64-linux.sig \\
          --certificate aegis-boot-x86_64-linux.pem \\
          aegis-boot-x86_64-linux
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
  end
end
