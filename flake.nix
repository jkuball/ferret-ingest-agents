{
  description = "Index coding agent sessions into ferret.";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    flake-parts.inputs.nixpkgs-lib.follows = "nixpkgs";
    import-tree.url = "github:denful/import-tree";
    rust-flake.url = "github:juspay/rust-flake";
    rust-flake.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = inputs: inputs.flake-parts.lib.mkFlake { inherit inputs; } {
    imports = [ (inputs.import-tree ./modules) ];

    systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
  };
}
