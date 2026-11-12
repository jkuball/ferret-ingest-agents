{ inputs, self, ... }:
{
  flake.presswerk.default = inputs.presswerk.lib.mkRelease {
    sourceRoot = self.outPath;
    versionFiles.cargo.ferret-ingest-agents = { };
    versionFiles.badge.default = { };
    changelog.path = "CHANGELOG.md";

    publishers.github-release.default = {
      repository = "jkuball/ferret-ingest-agents";
      assets = {
        "ferret-ingest-agents-aarch64-darwin.tar.gz" = self.packages.aarch64-darwin.release-archive;
        "ferret-ingest-agents-x86_64-linux.tar.gz" = self.packages.x86_64-linux.release-archive;
        "ferret-ingest-agents-aarch64-linux.tar.gz" = self.packages.aarch64-linux.release-archive;
      };
    };
  };
}
