{
  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1";
    flake-parts.url = "github:hercules-ci/flake-parts";
    flake-parts.inputs.nixpkgs-lib.follows = "nixpkgs";
    git-hooks.url = "github:cachix/git-hooks.nix";
    git-hooks.inputs.nixpkgs.follows = "nixpkgs";
    treefmt-nix.url = "github:numtide/treefmt-nix";
    treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      nixpkgs,
      flake-parts,
      git-hooks,
      treefmt-nix,
      ...
    }@inputs:
    flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [
        treefmt-nix.flakeModule
        git-hooks.flakeModule
      ];

      flake = { };

      perSystem =
        {
          config,
          pkgs,
          system,
          ...
        }:
        {
          _module.args = {
            pkgs = import nixpkgs {
              inherit system;
            };
          };

          apps = { };

          checks = { };

          devShells = {
            default = pkgs.mkShellNoCC {
              inputsFrom = [ config.pre-commit.devShell ];
            };
          };

          packages = { };

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
