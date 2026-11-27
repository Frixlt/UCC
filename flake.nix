{
  description = "Uniwill Control Center (UCC)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";

    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      treefmt-nix,
    }:
    let
      overlay = final: _prev: {
        ucc = final.callPackage ./package.nix { src = self; };
      };
    in
    {
      overlays.default = overlay;
      nixosModules.uccd = import ./nix/nixos-module.nix { inherit overlay; };
      nixosModules.default = self.nixosModules.uccd;
    }
    // flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ overlay ];
        };

        treefmtEval = treefmt-nix.lib.evalModule pkgs ./treefmt.nix;
      in
      {
        packages.ucc = pkgs.ucc;
        packages.default = pkgs.ucc;

        # `nix fmt`
        formatter = treefmtEval.config.build.wrapper;

        # `nix flake check`
        checks = {
          inherit (pkgs) ucc;
          formatting = treefmtEval.config.build.check self;
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ pkgs.ucc ];

          packages = with pkgs; [
            cmake
            pkg-config

            # Nix lint + format toolbox — matches what CI runs
            deadnix
            nixfmt
            statix
            treefmtEval.config.build.wrapper
          ];
        };
      }
    );
}
