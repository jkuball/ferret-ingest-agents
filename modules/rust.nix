{ inputs, lib, self, ... }:
{
  imports = [
    inputs.rust-flake.flakeModules.default
    inputs.rust-flake.flakeModules.nixpkgs
  ];

  perSystem = { config, self', ... }: {
    # The built-in templates are compiled in, so they belong to the source.
    rust-project.src = lib.cleanSourceWith {
      src = self;
      filter = path: type:
        lib.hasSuffix ".j2" path
        || config.rust-project.crane-lib.filterCargoSources path type;
    };

    packages.default = self'.packages.ferret-ingest-agents;
  };
}
