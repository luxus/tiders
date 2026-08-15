{
  description = "Tiders — a terminal (TUI + CLI) client for TIDAL";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      # nixpkgs 26.11 dropped x86_64-darwin; the overlay still builds there
      # when applied to an older nixpkgs (e.g. 26.05).
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = lib.genAttrs systems;
      pkgsFor =
        system:
        import nixpkgs {
          inherit system;
          overlays = [ self.overlays.default ];
        };
    in
    {
      overlays.default = final: _prev: {
        tiders = final.callPackage ./nix/package.nix { };
      };

      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          inherit (pkgs) tiders;
          default = pkgs.tiders;
        }
      );

      apps = forAllSystems (
        system:
        rec {
          default = {
            type = "app";
            program = lib.getExe self.packages.${system}.tiders;
          };
          tiders = default;
        }
      );

      checks = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          tiders = self.packages.${system}.tiders;
        in
        {
          inherit tiders;

          # Exercises the installed, PATH-wrapped binary (mpv on PATH).
          cli = pkgs.runCommand "tiders-cli" { nativeBuildInputs = [ tiders ]; } ''
            set -euo pipefail
            tiders --help
            tiders --version
            mkdir -p "$out"
          '';
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ pkgs.tiders ];
            packages = with pkgs; [
              rustc
              cargo
              clippy
              rustfmt
              rust-analyzer
              mpv
            ];
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          };
        }
      );

      formatter = forAllSystems (system: (pkgsFor system).nixfmt);
    };
}
