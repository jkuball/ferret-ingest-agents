{ inputs, self, ... }:
{
  imports = [ inputs.flake-parts.flakeModules.modules ];

  # The conventional outputs, so consumers needn't know about flake-parts.
  flake = {
    homeModules = self.modules.homeManager;
    nixosModules = self.modules.nixos;
    darwinModules = self.modules.darwin;
  };
}
