class DtachRs < Formula
  desc "Cross-platform Rust clone of dtach for detachable terminal sessions"
  homepage "https://github.com/nicoavanzdev/dtach-rs"
  license "MIT"
  version "0.1.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/nicoavanzdev/dtach-rs/releases/download/v#{version}/dtach-rs-aarch64-apple-darwin.tar.gz"
      sha256 "48dd3b3627693c95e45b52a6199570c4b0f5cc93b4cc64f5c491c0299593f8e1"
    else
      url "https://github.com/nicoavanzdev/dtach-rs/releases/download/v#{version}/dtach-rs-x86_64-apple-darwin.tar.gz"
      sha256 "054edfaa4a31ec824c45a9007a2f764c17cddb84d7374004bbb2f3376f531b56"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/nicoavanzdev/dtach-rs/releases/download/v#{version}/dtach-rs-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "1fc61e6e6ec6ab4fe47a5db58f75201a9c520eb2c5b1d91a70a9bc4ff913cb9c"
    else
      url "https://github.com/nicoavanzdev/dtach-rs/releases/download/v#{version}/dtach-rs-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "25834b2e5cc7ed43162b8b3e3c57dd701ea9782d1e088b6e8ac6d21edc92a774"
    end
  end

  def install
    bin.install "dtach-rs"
  end

  test do
    system "#{bin}/dtach-rs", "--help"
  end
end
