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
      name = crate.cargoToml.package.name;
      version = crate.cargoToml.package.version;
      description = crate.cargoToml.package.description;
      linuxTarget = {
        x86_64-linux = "x86_64-unknown-linux-musl";
        aarch64-linux = "aarch64-unknown-linux-musl";
      }.${system} or null;
      targetKey = lib.toUpper (lib.replaceStrings [ "-" ] [ "_" ] (if linuxTarget == null then "" else linuxTarget));
      linker = lib.getExe pkgs.pkgsStatic.stdenv.cc;
      targetArgs = lib.optionalAttrs (linuxTarget != null) {
        CARGO_BUILD_TARGET = linuxTarget;
        "CARGO_TARGET_${targetKey}_LINKER" = linker;
        "CC_${lib.replaceStrings [ "-" ] [ "_" ] linuxTarget}" = linker;
        nativeBuildInputs = [ pkgs.pkgsStatic.stdenv.cc ];
      };
      commonArgs = crate.crane.args // targetArgs // {
        inherit version;
        pname = name;
        src = config.rust-project.src;
        cargoExtraArgs = "-p ${name}";
        inherit cargoVendorDir;
        strictDeps = true;
      };
      dummyBuild = pkgs.writeText "dummy-build.rs" "fn main() {}\n";
      normalizedCargoToml = (pkgs.formats.toml { }).generate "Cargo.toml" (
        crate.cargoToml // {
          package = crate.cargoToml.package // {
            build = dummyBuild;
            version = "0.0.0";
          };
        }
      );
      cargoLock = builtins.fromTOML (builtins.readFile (self + /Cargo.lock));
      localPackages = builtins.filter (package: package.name == name && !(package ? source)) cargoLock.package;
      normalizedCargoLockData = cargoLock // {
        package = map
          (package:
            if package.name == name && !(package ? source) then
              package // { version = "0.0.0"; }
            else
              package)
          cargoLock.package;
      };
      normalizedCargoLock = (pkgs.formats.toml { }).generate "Cargo.lock" normalizedCargoLockData;
      cargoVendorDir = craneLib.vendorCargoDeps {
        cargoLockParsed = normalizedCargoLockData;
      };
      dependencySource = pkgs.runCommand "source" { } ''
        mkdir -p "$out/src"
        cp ${normalizedCargoToml} "$out/Cargo.toml"
        cp ${normalizedCargoLock} "$out/Cargo.lock"
        printf 'fn main() {}\n' > "$out/src/main.rs"
      '';
      cargoArtifacts = craneLib.buildDepsOnly (builtins.removeAttrs commonArgs [ "src" ] // {
        dummySrc = dependencySource;
        version = "0.0.0";
      });
      package = craneLib.buildPackage (commonArgs // {
        inherit cargoArtifacts;
        meta = { inherit description; };
      } // lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
        postInstall = ''
          iconv=$(${pkgs.darwin.cctools}/bin/otool -L "$out/bin/${name}" \
            | awk '/\/nix\/store\/.*libiconv/{ print $1 }')
          if test -n "$iconv"; then
            ${pkgs.darwin.cctools}/bin/install_name_tool \
              -change "$iconv" /usr/lib/libiconv.2.dylib "$out/bin/${name}"
          fi
        '';
      });
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
          echo "$description" | grep -qi 'statically linked'
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
      dependencyCheck = assert builtins.length localPackages == 1;
        assert cargoArtifacts.version == "0.0.0";
        pkgs.runCommand "${name}-dependency-cache-check" { } ''
          grep -q '^version = "0.0.0"$' ${dependencySource}/Cargo.toml
          grep -A1 '^name = "${name}"$' ${dependencySource}/Cargo.lock \
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
