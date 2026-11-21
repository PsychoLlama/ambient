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
        pkgs.fenix.combine (
          [
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
          ]
          # A release for Linux is linked statically, so that it runs on a
          # machine with no Nix store: see `.github/workflows/release.yml`.
          ++
            lib.optional pkgs.stdenv.hostPlatform.isLinux
              pkgs.fenix.targets."${pkgs.stdenv.hostPlatform.qemuArch}-unknown-linux-musl".stable.rust-std
        );

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
              # Runs the JavaScript host's tests: `crates/ambient-js`.
              pkgs.nodejs
              pkgs.prettier
              pkgs.treefmt
            ];
          };

          # Tools that only matter when a human (or coding agent) is actively
          # iterating on the source.
          coding = pkgs.mkShell {
            inputsFrom = [ default ];

            # Each worktree builds into its own `target`. One shared by
            # all of them is not sound: cargo names a crate's artifacts
            # by its path within the workspace and takes them as fresh
            # by their time, so a checkout is handed what another built
            # from other source.
          };
        }
      );
    };
}
