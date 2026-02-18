{
  description = "ESP32-H2 / ESP32-C6 Rust dev shell";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-25.11";
    rust-overlay.url = "github:oxalica/rust-overlay";

    nixvim = {
      url  = "github:nix-community/nixvim?ref=nixos-25.11";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    nixvim-config = {
      url = "path:/home/winteru/Documents/nixvim";
    };
  };

  outputs = { nixpkgs, nixvim, nixvim-config, rust-overlay, ... }:
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
        nixvimPkgs = nixvim.legacyPackages.${system};
        base = nixvim-config.nixvimModules.base { inherit pkgs; };
        nvim = nixvimPkgs.makeNixvimWithModule {
            inherit system;
            module = nixpkgs.lib.recursiveUpdate base {
                # Extend de la config de base 
                plugins.lsp.enable = true;
                plugins.rustaceanvim.settings.server.default_settings."rust-analyzer".check = {
                    command = "clippy";
                    extraArgs = [
                        "--no-default-features"
                    ];
                    allTargets = false;
                };
            };
        };
    in {
      default = pkgs.mkShell {
        packages = with pkgs; [
          nvim
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

