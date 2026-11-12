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
      nixpkgs,
      fenix,
      systems,
      ...
    }:

    let
      inherit (nixpkgs) lib;

      overlays = [ fenix.overlays.default ];

      # Shared toolchain for every build and the dev shell. `rust-src` lets
      # rust-analyzer load the standard library from the sysroot; `rust-analyzer`
      # rides along so the editor resolves against this exact toolchain.
      rustToolchainFor =
        pkgs:
        pkgs.fenix.combine [
          (pkgs.fenix.stable.withComponents [
            "cargo"
            "clippy"
            "rust-analyzer"
            "rust-src"
            "rustc"
            "rustfmt"
          ])
          # The runtime is also built for WebAssembly, to run beside the
          # code the WebAssembly backend emits.
          pkgs.fenix.targets.wasm32-unknown-unknown.stable.rust-std
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
      devShells = eachSystem (
        system: pkgs: rec {
          # Common environment used in development and CI.
          default = pkgs.mkShell {
            packages = [
              (rustToolchainFor pkgs)

              pkgs.cargo-nextest
              pkgs.nixfmt
              pkgs.prettier
              pkgs.treefmt
            ];
          };

          # Tools that only matter when a human (or coding agent) is actively
          # iterating on the source.
          coding = pkgs.mkShell {
            inputsFrom = [ default ];

            # Share the Rust build cache across worktrees.
            # Only local - not set in CI.
            shellHook = ''
              if _git_common="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)"; then
                export CARGO_TARGET_DIR="$(dirname "$_git_common")/target"
                unset _git_common
              fi
            '';
          };
        }
      );
    };
}
