{
  description = "Ambient programming language";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    systems.url = "github:nix-systems/default";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      systems,
    }:

    let
      inherit (nixpkgs) lib;

      overlays = [ (import rust-overlay) ];

      eachSystem = lib.flip lib.mapAttrs (
        lib.genAttrs (import systems) (
          system:
          import nixpkgs {
            inherit system overlays;
          }
        )
      );
    in

    {
      packages = eachSystem (
        system: pkgs:
        let
          rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };
        in
        {
          ambient = rustPlatform.buildRustPackage {
            pname = "ambient";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            meta = {
              description = "Ambient programming language CLI";
              mainProgram = "ambient";
            };
          };

          tree-sitter-ambient = pkgs.stdenv.mkDerivation {
            pname = "tree-sitter-ambient";
            version = "0.1.0";
            src = ./tree-sitter-ambient;

            nativeBuildInputs = [
              pkgs.tree-sitter
              pkgs.nodejs
            ];

            buildPhase = ''
              tree-sitter generate
            '';

            installPhase = ''
              mkdir -p $out
              cp -r src $out/
              cp -r queries $out/
              cp grammar.js $out/
              cp package.json $out/
            '';

            meta = {
              description = "Tree-sitter grammar for the Ambient programming language";
            };
          };

          default = self.packages.${system}.ambient;
        }
      );

      devShells = eachSystem (
        system: pkgs: {
          default = pkgs.mkShell {
            packages = [
              (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
              pkgs.just
              pkgs.tree-sitter
              pkgs.nodejs
              self.packages.${system}.ambient
            ];
          };
        }
      );
    };
}
