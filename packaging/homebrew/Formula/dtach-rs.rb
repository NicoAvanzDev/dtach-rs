class DtachRs < Formula
  desc "Cross-platform Rust clone of dtach for detachable terminal sessions"
  homepage "https://github.com/nicoavanzdev/dtach-rs"
  license "MIT"
  version "0.1.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/nicoavanzdev/dtach-rs/releases/download/v#{version}/dtach-rs-aarch64-apple-darwin.tar.gz"
      sha256 "<FILL_IN_AFTER_RELEASE>"
    else
      url "https://github.com/nicoavanzdev/dtach-rs/releases/download/v#{version}/dtach-rs-x86_64-apple-darwin.tar.gz"
      sha256 "<FILL_IN_AFTER_RELEASE>"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/nicoavanzdev/dtach-rs/releases/download/v#{version}/dtach-rs-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "<FILL_IN_AFTER_RELEASE>"
    else
      url "https://github.com/nicoavanzdev/dtach-rs/releases/download/v#{version}/dtach-rs-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "<FILL_IN_AFTER_RELEASE>"
    end
  end

  def install
    bin.install "dtach-rs"
  end

  test do
    system "#{bin}/dtach-rs", "--help"
  end
end
