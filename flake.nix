{
  description = "Reproducible development environment for quick_resequencing";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
  };

  outputs = { self, nixpkgs }:
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
          pkgs = import nixpkgs { inherit system; };
          python = pkgs.python312.withPackages (
            ps: with ps; [
              matplotlib
            ]
          );
        in {
          default = pkgs.mkShell {
            packages = [
              python
              pkgs.rustc
              pkgs.cargo
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
