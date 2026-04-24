# Homebrew formula for aegis-boot.
#
# Install:
#   brew tap aegis-boot/aegis-boot https://github.com/aegis-boot/aegis-boot
#   brew install aegis-boot
#
# Supported platform: macOS Apple Silicon (arm64) only.
#
# Linux + Windows + macOS Intel operators: aegis-boot's primary
# deliverable is the signed `.img` bootable USB image, not the CLI
# binary. Native channels for those platforms:
#   - Linux:    scripts/install.sh (cosign-verified), cargo install,
#               or the distro's package manager.
#   - Windows:  download the `.img` from GitHub Releases and flash
#               with Rufus (https://rufus.ie). No Windows CLI needed.
#   - macOS Intel: deferred (tracked in #365).
#
# Why brew is macOS-only here (per consensus vote on brew shrink):
#   - macOS operators expect `brew install`; it's their canonical
#     CLI-install channel + gives `brew upgrade` / `brew uninstall`.
#   - Linux operators have better native channels (apt/dnf/cargo),
#     so a Linux brew bottle was dead weight.
class AegisBoot < Formula
  desc "Signed UEFI Secure Boot rescue environment for booting any ISO from USB"
  homepage "https://github.com/aegis-boot/aegis-boot"
  version "0.17.0"
  license any_of: ["Apache-2.0", "MIT"]

  # Runtime dependencies the operator CLI shells out to. Listed
  # explicitly so brew installs them; aegis-boot doctor will also
  # verify them at runtime.
  depends_on "curl"
  depends_on "gnupg"
  depends_on "gptfdisk" # provides sgdisk
  depends_on :macos

  on_macos do
    on_arm do
      # macOS arm64 binary ships with every release (#365 Phase A1).
      # The sha256 below is bumped per-release by the bump-brew-formula
      # job in release.yml.
      url "https://github.com/aegis-boot/aegis-boot/releases/download/v0.17.0/aegis-boot-aarch64-apple-darwin"
      sha256 "ceea2b35d966bfe73387f3bf86f61e2a93f320d17430e61ba747dbc5ddd415a1"
    end
  end

  def install
    if OS.mac? && Hardware::CPU.arm?
      bin.install "aegis-boot-aarch64-apple-darwin" => "aegis-boot"
    else
      odie <<~EOS
        aegis-boot's Homebrew formula publishes only the macOS Apple
        Silicon (arm64) binary.

        Linux operators: use the cosign-verified install script,
        `cargo install`, or your distro's package manager. See:
          https://github.com/aegis-boot/aegis-boot/blob/main/docs/INSTALL.md

        Windows operators: download the `.img` from GitHub Releases
        and flash with Rufus (https://rufus.ie). No CLI install
        needed for the typical operator path.

        macOS Intel: deferred. Build from source if needed:
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
    binary_name = "aegis-boot-aarch64-apple-darwin"
    <<~EOS
      aegis-boot is a USB-imaging tool for macOS operator workstations.

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
