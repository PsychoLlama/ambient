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
          rustToolchain = pkgs.fenix.stable.defaultToolchain;
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

            # Skip PTY integration tests in nix build (dynamic linker issues in sandbox)
            checkFlags = [
              "--skip=test_basic_arithmetic"
              "--skip=test_boolean_literal"
              "--skip=test_clear_command"
              "--skip=test_completion_no_snippet_syntax"
              "--skip=test_console_dot_completion_preserves_prefix"
              "--skip=test_core_string_methods_completion"
              "--skip=test_ctrl_c_interrupt"
              "--skip=test_define_and_call_function"
              "--skip=test_define_constant"
              "--skip=test_help_command"
              "--skip=test_history_up_arrow"
              "--skip=test_multiplication"
              "--skip=test_parse_error"
              "--skip=test_tab_completion_console"
              "--skip=test_tab_completion_keyword"
              "--skip=test_undefined_variable"
              "--skip=test_unterminated_string_does_not_crash"
              "--skip=test_core_list_shadow_suggestion_shows_only_suffix"
              "--skip=test_core_list_dot_shows_function_completions"
              "--skip=test_core_list_first_inspects_as_function"
              "--skip=test_dotted_module_path_is_rejected"
              "--skip=test_user_defined_function_inspection"
            ];

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

          default = self.packages.${system}.ambient;
        }
      );

      devShells = eachSystem (
        system: pkgs: {
          default = pkgs.mkShell {
            packages = [
              pkgs.fenix.stable.defaultToolchain
              pkgs.fenix.stable.rust-analyzer
              pkgs.just
              pkgs.nixfmt
              pkgs.nodejs
              pkgs.stylua
              pkgs.tree-sitter
              pkgs.treefmt
            ];

            TREE_SITTER_AMBIENT = self.packages.${system}.tree-sitter-ambient;
          };
        }
      );
    };
}
