{ inputs, lib, self, ... }:
{
  imports = [
    inputs.rust-flake.flakeModules.default
    inputs.rust-flake.flakeModules.nixpkgs
  ];

  perSystem =
    { config, pkgs, self', system, ... }:
    let
      crate = config.rust-project.crates.ferret-ingest-agents;
      craneLib = config.rust-project.crane-lib;
      inherit (crate.cargoToml.package) name version description;

      # Linux builds produce static musl binaries; Darwin builds natively.
      linuxTarget = {
        x86_64-linux = "x86_64-unknown-linux-musl";
        aarch64-linux = "aarch64-unknown-linux-musl";
      }.${system} or null;
      targetArgs = lib.optionalAttrs (linuxTarget != null) (
        let
          linker = lib.getExe pkgs.pkgsStatic.stdenv.cc;
          targetEnv = lib.replaceStrings [ "-" ] [ "_" ] linuxTarget;
        in
        {
          CARGO_BUILD_TARGET = linuxTarget;
          "CARGO_TARGET_${lib.toUpper targetEnv}_LINKER" = linker;
          "CC_${targetEnv}" = linker;
          nativeBuildInputs = [ pkgs.pkgsStatic.stdenv.cc ];
        }
      );

      # Dependencies are built from manifests that pin this crate to 0.0.0,
      # so a version bump alone does not invalidate the dependency cache.
      dependencies =
        let
          isLocal = package: package.name == name && !(package ? source);
          cargoLock = builtins.fromTOML (builtins.readFile (self + /Cargo.lock));
          normalizedCargoLockData = cargoLock // {
            package = map
              (package:
                if isLocal package then
                  package // { version = "0.0.0"; }
                else
                  package)
              cargoLock.package;
          };
          normalizedCargoLock = (pkgs.formats.toml { }).generate "Cargo.lock" normalizedCargoLockData;
          dummyBuild = pkgs.writeText "dummy-build.rs" "fn main() {}\n";
          normalizedCargoToml = (pkgs.formats.toml { }).generate "Cargo.toml" (
            crate.cargoToml // {
              package = crate.cargoToml.package // {
                build = dummyBuild;
                version = "0.0.0";
              };
            }
          );
        in
        {
          localPackages = builtins.filter isLocal cargoLock.package;
          vendorDir = craneLib.vendorCargoDeps {
            cargoLockParsed = normalizedCargoLockData;
          };
          source = pkgs.runCommand "source" { } ''
            mkdir -p "$out/src"
            cp ${normalizedCargoToml} "$out/Cargo.toml"
            cp ${normalizedCargoLock} "$out/Cargo.lock"
            printf 'fn main() {}\n' > "$out/src/main.rs"
          '';
        };

      commonArgs = crate.crane.args // targetArgs // {
        inherit version;
        pname = name;
        src = config.rust-project.src;
        cargoExtraArgs = "-p ${name}";
        cargoVendorDir = dependencies.vendorDir;
        strictDeps = true;
      };
      cargoArtifacts = craneLib.buildDepsOnly (builtins.removeAttrs commonArgs [ "src" ] // {
        dummySrc = dependencies.source;
        version = "0.0.0";
      });

      # Link Darwin binaries against the system libiconv so they run outside Nix.
      portableDarwinArgs = lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
        postInstall = ''
          iconv=$(${pkgs.darwin.cctools}/bin/otool -L "$out/bin/${name}" \
            | awk '/\/nix\/store\/.*libiconv/{ print $1 }')
          if test -n "$iconv"; then
            ${pkgs.darwin.cctools}/bin/install_name_tool \
              -change "$iconv" /usr/lib/libiconv.2.dylib "$out/bin/${name}"
          fi
        '';
      };
      package = craneLib.buildPackage (commonArgs // {
        inherit cargoArtifacts;
        meta = { inherit description; };
      } // portableDarwinArgs);

      # Release archives hold just the binary and are checked as shipped.
      archive = pkgs.runCommand "${name}-${system}.tar.gz" { nativeBuildInputs = [ pkgs.gnutar ]; } ''
        tar --sort=name --mtime=@1 --owner=0 --group=0 --numeric-owner \
          -czf "$out" -C ${package}/bin ${name}
      '';
      archiveCheck = pkgs.runCommand "${name}-${system}-release-check"
        {
          nativeBuildInputs = [
            pkgs.file
            pkgs.gnutar
          ] ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.binutils ]
          ++ lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.darwin.cctools ];
        } ''
        entries=$(tar -tzf ${archive})
        test "$entries" = ${lib.escapeShellArg name}

        mkdir extracted
        tar -xzf ${archive} -C extracted
        binary="$PWD/extracted/${name}"
        test -f "$binary"
        test -x "$binary"
        test "$("$binary" --version)" = ${lib.escapeShellArg "${name} ${version}"}

        description=$(file -b "$binary")
        echo "$description"
        ${lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
          echo "$description" | grep -Eqi 'statically linked|static(-pie)? linked'
          ! readelf -d "$binary" | grep -q '(NEEDED)'
        ''}
        ${lib.optionalString pkgs.stdenv.hostPlatform.isDarwin ''
          links=$(otool -L "$binary")
          echo "$links"
          ! echo "$links" | grep -q /nix/store
          non_system=$(echo "$links" | tail -n +2 | awk '{ print $1 }' \
            | grep -Ev '^(/usr/lib/|/System/Library/)' || true)
          test -z "$non_system"
        ''}
        touch "$out"
      '';

      dependencyCheck = assert builtins.length dependencies.localPackages == 1;
        assert cargoArtifacts.version == "0.0.0";
        pkgs.runCommand "${name}-dependency-cache-check" { } ''
          grep -q '^version = "0.0.0"$' ${dependencies.source}/Cargo.toml
          grep -A1 '^name = "${name}"$' ${dependencies.source}/Cargo.lock \
            | grep -q '^version = "0.0.0"$'
          touch "$out"
        '';
    in
    {
      # The built-in templates are compiled in, so they belong to the source.
      rust-project = {
        src = lib.cleanSourceWith {
          src = self;
          filter = path: type:
            lib.hasSuffix ".j2" path
            || config.rust-project.crane-lib.filterCargoSources path type;
        };
        toolchain = (pkgs.rust-bin.fromRustupToolchainFile (self + /rust-toolchain.toml)).override {
          extensions = [
            "clippy"
            "rust-analyzer"
            "rust-src"
          ];
          targets = lib.optional (linuxTarget != null) linuxTarget;
        };
        crates.ferret-ingest-agents.crane.outputs.drv = {
          crate = package;
          clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets --all-features -- --deny warnings";
            meta.description = "Clippy check for the ${name} crate";
          });
          doc = craneLib.cargoDoc (commonArgs // {
            inherit cargoArtifacts;
            RUSTDOCFLAGS = "-D warnings";
            meta.description = "Rust docs for the ${name} crate";
          });
        };
      };

      packages = {
        default = self'.packages.ferret-ingest-agents;
        release-archive = archive;
      };

      checks = {
        dependency-cache = dependencyCheck;
        release-archive = archiveCheck;
      };
    };
}
