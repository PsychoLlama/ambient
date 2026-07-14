{
  description = "Ambient programming language";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    systems.url = "github:nix-systems/default";

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      fenix,
      systems,
    }:

    let
      inherit (nixpkgs) lib;

      overlays = [ fenix.overlays.default ];

      # Shared toolchain for every build and the dev shell. `rust-src` lets
      # rust-analyzer load the standard library from the sysroot; `rust-analyzer`
      # rides along so the editor resolves against this exact toolchain.
      rustToolchainFor =
        pkgs:
        pkgs.fenix.stable.withComponents [
          "cargo"
          "clippy"
          "rust-analyzer"
          "rust-src"
          "rustc"
          "rustfmt"
        ];

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
        system: pkgs: {
          tree-sitter-ambient = pkgs.stdenv.mkDerivation {
            pname = "tree-sitter-ambient";
            version = "0.1.0";
            src = ./tree-sitter-ambient;

            nativeBuildInputs = [
              pkgs.tree-sitter
              pkgs.nodejs
              pkgs.pnpm
            ];

            buildPhase = ''
              tree-sitter generate
              mkdir -p parser
              $CC -shared -fPIC -o parser/ambient.so -I src src/parser.c
            '';

            installPhase = ''
              mkdir -p $out $out/parser
              cp -r src $out/
              cp -r queries $out/
              cp grammar.js $out/
              cp package.json $out/
              cp parser/ambient.so $out/parser/
            '';

            meta = {
              description = "Tree-sitter grammar for the Ambient programming language";
            };
          };

          ambient-nvim =
            let
              tree-sitter-grammar = self.packages.${system}.tree-sitter-ambient;
            in
            pkgs.vimUtils.buildVimPlugin {
              pname = "ambient.nvim";
              version = "0.1.0";
              src = ./ambient.nvim;

              nativeBuildInputs = [ pkgs.gcc ];

              postInstall = ''
                # Build the tree-sitter parser shared library
                mkdir -p $out/parser
                $CC -shared -fPIC -o $out/parser/ambient.so \
                  -I${tree-sitter-grammar}/src \
                  ${tree-sitter-grammar}/src/parser.c
              '';

              meta = {
                description = "Neovim plugin for the Ambient programming language";
                homepage = "https://github.com/psychollama/ambient";
              };
            };
        }
      );

      devShells = eachSystem (
        system: pkgs: rec {
          # Common environment used in development and CI.
          default = pkgs.mkShell {
            packages = [
              (rustToolchainFor pkgs)

              pkgs.cargo-nextest
              pkgs.just
              pkgs.nixfmt
              pkgs.nodejs
              pkgs.pnpm
              pkgs.prettier
              pkgs.stylua
              pkgs.tree-sitter
              pkgs.treefmt
            ];
          };

          # Tools that only matter when a human (or coding agent) is actively
          # iterating on the source.
          coding = pkgs.mkShell {
            inputsFrom = [ default ];

            packages = [
              (pkgs.writeShellApplication {
                name = "ambient";
                text = ''
                  cargo run --quiet --package ambient-cli -- "$@"
                '';
              })
            ];

            # Share the Rust build cache across worktrees.
            # Only local - not set in CI.
            shellHook = ''
              if _git_common="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)"; then
                export CARGO_TARGET_DIR="$(dirname "$_git_common")/target"
                unset _git_common
              fi
            '';

            TREE_SITTER_AMBIENT = self.packages.${system}.tree-sitter-ambient;
          };
        }
      );
    };
}
