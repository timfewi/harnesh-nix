{
  description = "harnesh: agent-agnostic tmux cockpit for running and supervising coding agents";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/eaad089433ca2bb662274377d33df3d0e51ef28b";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          harnesh = nixpkgs.legacyPackages.${system}.callPackage ./nix/package.nix { };
        in
        {
          inherit harnesh;
          default = harnesh;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = nixpkgs.lib.getExe self.packages.${system}.harnesh;
          meta.description = "Run the harnesh agent cockpit";
        };
      });

      overlays.default = final: _prev: {
        harnesh = final.callPackage ./nix/package.nix { };
      };

      nixosModules.default = import ./nix/module.nix { inherit self; };
      nixosModules.harnesh = self.nixosModules.default;

      # Agent Skills-compatible directories, for harnesses that load skills.
      lib.skills.harnesh-supervisor = ./skills/harnesh-supervisor;

      checks = forAllSystems (system: import ./nix/checks.nix { inherit self nixpkgs system; });

      devShells = forAllSystems (system: {
        default = nixpkgs.legacyPackages.${system}.mkShell {
          packages = with nixpkgs.legacyPackages.${system}; [
            bashInteractive
            cargo
            clippy
            coreutils
            deadnix
            git
            jq
            just
            nixfmt
            ripgrep
            rust-analyzer
            rustc
            rustfmt
            statix
            tmux
          ];
        };
      });

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt-tree);
    };
}
