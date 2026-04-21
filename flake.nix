{
  description = "Reproducible development environment for quick_resequencing";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in {
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [
              rust-overlay.overlays.default
            ];
          };
          python = pkgs.python312.withPackages (
            ps: with ps; [
              matplotlib
            ]
          );
          rustToolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rustfmt"
              "clippy"
            ];
          };
        in {
          default = pkgs.mkShell {
            packages = [
              python
              rustToolchain
              pkgs.pkg-config
              pkgs.git
            ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
              pkgs.linuxPackages.perf
            ];

            shellHook = ''
              export QUICK_RESEQUENCING_NIX_DEV=1
              export PYTHONNOUSERSITE=1
              echo "Entered quick_resequencing dev shell"
              echo "python: $(python3 --version)"
              echo "rust: $(rustc --version)"
            '';
          };
        }
      );
    };
}
