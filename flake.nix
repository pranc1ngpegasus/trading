{
  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1";
    flake-parts.url = "github:hercules-ci/flake-parts";
    flake-parts.inputs.nixpkgs-lib.follows = "nixpkgs";
    git-hooks.url = "github:cachix/git-hooks.nix";
    git-hooks.inputs.nixpkgs.follows = "nixpkgs";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    crane.url = "github:ipetkov/crane";
    advisory-db.url = "github:rustsec/advisory-db";
    advisory-db.flake = false;
    mutator.url = "github:mokmok-dev/mutator";
    mutator.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      nixpkgs,
      flake-parts,
      git-hooks,
      treefmt-nix,
      rust-overlay,
      crane,
      advisory-db,
      mutator,
      ...
    }@inputs:
    flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [
        treefmt-nix.flakeModule
        git-hooks.flakeModule
        mutator.flakeModules.default
      ];

      flake = { };

      perSystem =
        {
          config,
          pkgs,
          system,
          ...
        }:
        let
          rustToolchain = pkgs.rust-bin.stable.latest.default;
          craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
          src = craneLib.cleanCargoSource ./.;
          commonArgs = {
            inherit src;
            # `tws-api` runs protoc and reads the proto tree in its build script.
            nativeBuildInputs = [ pkgs.protobuf ];
            strictDeps = true;
            TWS_API_ROOT = "${twsApi}/share/tws-api";
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          # `cargo-mutants` lets one compiler task run per CPU, so a mutant build
          # spawns that many `rustc` processes and peak memory follows the core
          # count. On this 32-core machine one `mutator --all` peaked at 485-500 MB
          # uncapped against 407-438 MB at four tasks; four concurrent runs peaked
          # at 1.87 GB against 1.61 GB. `tws-api` compiles 200 protos into one
          # crate, so a single `rustc` on it costs about 430 MB. A run that passes
          # its own jobserver flags keeps them, and `CARGO_MUTANTS_JOBSERVER_TASKS`
          # overrides the cap.
          cargoMutants = pkgs.writeShellScriptBin "cargo-mutants" ''
            real=${pkgs.cargo-mutants}/bin/cargo-mutants
            tasks=''${CARGO_MUTANTS_JOBSERVER_TASKS:-4}
            # Never raise the limit on a machine with fewer cores than the cap.
            cores=$(${pkgs.coreutils}/bin/nproc)
            if [ "$cores" -lt "$tasks" ]; then
              tasks=$cores
            fi
            for arg in "$@"; do
              case "$arg" in
                --jobserver | --jobserver=* | --jobserver-tasks | --jobserver-tasks=*)
                  exec "$real" "$@"
                  ;;
              esac
            done
            # `--` opens cargo test's arguments, where a `cargo mutants` flag is
            # a test argument instead.
            args=()
            inserted=""
            for arg in "$@"; do
              if [ "$arg" = "--" ] && [ -z "$inserted" ]; then
                args+=(--jobserver-tasks "$tasks")
                inserted=yes
              fi
              args+=("$arg")
            done
            [ -n "$inserted" ] || args+=(--jobserver-tasks "$tasks")
            exec "$real" "''${args[@]}"
          '';

          linkerPackages = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (
            with pkgs;
            [
              clang
              mold
            ]
          );
          linkerEnv = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
            export RUSTFLAGS="-C link-arg=-fuse-ld=mold"
            export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="clang"
            export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="clang"
          '';

          twsApi = pkgs.callPackage ./nix/tws-api.nix { };
        in
        {
          _module.args = {
            pkgs = import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            };
          };

          apps = { };

          checks = {
            audit = craneLib.cargoAudit (
              commonArgs
              // {
                inherit advisory-db;
              }
            );
            clippy = craneLib.cargoClippy (
              commonArgs
              // {
                inherit cargoArtifacts;
                cargoClippyExtraArgs = "--all-targets --all-features -- --deny warnings";
              }
            );
            deny = craneLib.cargoDeny commonArgs;
            fmt = craneLib.cargoFmt (
              commonArgs
              // {
                inherit cargoArtifacts;
              }
            );
            mutator = config.packages.mutator;
            taplo = craneLib.taploFmt commonArgs;
            test = craneLib.cargoTest (
              commonArgs
              // {
                inherit cargoArtifacts;
              }
            );
          };

          devShells = {
            default = pkgs.mkShellNoCC {
              inputsFrom = [ config.pre-commit.devShell ];

              packages = with pkgs; [
                cargoMutants
                linkerPackages
                mutator.packages.${system}.default
                protobuf
                rustToolchain
                sccache
                twsApi
              ];

              shellHook = ''
                export RUSTC_WRAPPER="${pkgs.sccache}/bin/sccache"
                export TWS_API_ROOT="${twsApi}/share/tws-api"
                ${linkerEnv}
              '';
            };
          };

          mutator = {
            enable = true;
            check = false;
          };

          packages = {
            inherit twsApi;
          };

          pre-commit.settings = {
            hooks = {
              actionlint.enable = true;
              deadnix.enable = true;
              statix.enable = true;
            };
            package = pkgs.prek;
          };

          treefmt = {
            projectRootFile = "flake.nix";
            programs = {
              nixfmt.enable = true;
              taplo.enable = true;
            };
          };
        };

      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
    };
}
