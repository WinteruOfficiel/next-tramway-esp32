{
  description = "ESP32-H2 / ESP32-C6 Rust dev shell";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-25.11";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { nixpkgs, rust-overlay, ... }:
  let
    allSystems = [
      "x86_64-linux"
      "aarch64-linux"
    ];

    mkShell = system:
    let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };

      rust = pkgs.rust-bin.nightly.latest.default.override {
        extensions = [ "rust-src" "rustfmt" "clippy" ];
        targets = [
          # ESP32-H2 / ESP32-C6
          "riscv32imc-unknown-none-elf"
        ];
      };
    in {
      default = pkgs.mkShell {
        packages = with pkgs; [
          # Rust
          rust
          cargo-binutils

          # Flash / debug
          espflash
          openocd
          gdb
          esp-generate

          # Others 
          libusb1
          pkg-config
          python3
        ];

        shellHook = ''
          export RUST_BACKTRACE=1
        '';
      };
    };
  in {
    devShells = nixpkgs.lib.genAttrs allSystems mkShell;
  };
}

