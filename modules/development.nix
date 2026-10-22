{
  perSystem = { pkgs, self', ... }: {
    devShells.default = pkgs.mkShell {
      inputsFrom = [ self'.devShells.rust ];
      packages = [ pkgs.nixpkgs-fmt ];
    };

    formatter = pkgs.nixpkgs-fmt;
  };
}
