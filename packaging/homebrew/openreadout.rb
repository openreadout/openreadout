# Homebrew formula TEMPLATE for OpenReadout.
#
# Do not install this file directly: the placeholders are filled from a release's SHA256SUMS by
#   cargo xtask homebrew-formula --version 0.1.0 --sums SHA256SUMS --out openreadout.rb
# The release workflow attaches the rendered openreadout.rb to every GitHub release; copy it to
# Formula/openreadout.rb in the openreadout/homebrew-tap repository to publish it.
#
# Bottle-less: it installs the prebuilt, statically linked release binary for this platform.
class Openreadout < Formula
  desc "Read raw lab-instrument files and export them to open formats"
  homepage "https://github.com/openreadout/openreadout"
  version "{{version}}"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/openreadout/openreadout/releases/download/v{{version}}/openreadout-aarch64-apple-darwin.tar.gz"
      sha256 "{{sha256:openreadout-aarch64-apple-darwin.tar.gz}}"
    end
    on_intel do
      url "https://github.com/openreadout/openreadout/releases/download/v{{version}}/openreadout-x86_64-apple-darwin.tar.gz"
      sha256 "{{sha256:openreadout-x86_64-apple-darwin.tar.gz}}"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/openreadout/openreadout/releases/download/v{{version}}/openreadout-aarch64-unknown-linux-musl.tar.gz"
      sha256 "{{sha256:openreadout-aarch64-unknown-linux-musl.tar.gz}}"
    end
    on_intel do
      url "https://github.com/openreadout/openreadout/releases/download/v{{version}}/openreadout-x86_64-unknown-linux-musl.tar.gz"
      sha256 "{{sha256:openreadout-x86_64-unknown-linux-musl.tar.gz}}"
    end
  end

  def install
    bin.install "openreadout"
    doc.install "README.md", "SKILL.md", "references"
    doc.install "THIRD-PARTY-NOTICES.md" if File.exist?("THIRD-PARTY-NOTICES.md")
    # Man pages and completions (`cargo xtask man`) ship in every release archive; the guards
    # keep an archive without them installable.
    man1.install Dir["man/*.1"] if Dir.exist?("man")
    if Dir.exist?("completions")
      bash_completion.install "completions/openreadout.bash" => "openreadout"
      zsh_completion.install "completions/_openreadout"
      fish_completion.install "completions/openreadout.fish"
    end
  end

  def caveats
    <<~EOS
      Agent skill:  openreadout self skill --install all
      MCP server:   openreadout mcp --config claude
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/openreadout --version")
    assert_match "czi", shell_output("#{bin}/openreadout self formats --json")
    assert_match "\"ok\": true", shell_output("#{bin}/openreadout self doctor --json")
  end
end
