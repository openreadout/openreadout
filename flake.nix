{
  description = "OpenReadout: read raw lab-instrument files (Zeiss CZI, Nikon ND2, Leica LIF) without vendor software";

  # nixos-unstable: the workspace needs Rust >= 1.91 (see rust-version in Cargo.toml).
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
    in
    {
      packages = forAllSystems (pkgs: rec {
        openreadout = pkgs.rustPlatform.buildRustPackage {
          pname = "openreadout";
          inherit (cargoToml.workspace.package) version;

          # Only what the Rust build needs, so doc or corpus edits do not trigger a rebuild.
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./LICENSE-MIT
              ./LICENSE-APACHE
              ./.cargo
              ./crates
              ./xtask
              ./skills
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;

          cargoBuildFlags = [ "-p" "openreadout" ];
          cargoTestFlags = [ "-p" "openreadout" ];

          meta = {
            description = "Read raw lab-instrument files (Zeiss CZI, Nikon ND2, Leica LIF) without vendor software";
            homepage = "https://github.com/openreadout/openreadout";
            license = with pkgs.lib.licenses; [ mit asl20 ];
            mainProgram = "openreadout";
            platforms = pkgs.lib.platforms.unix;
          };
        };
        default = openreadout;
      });

      apps = forAllSystems (pkgs: rec {
        openreadout = {
          type = "app";
          program = pkgs.lib.getExe self.packages.${pkgs.stdenv.hostPlatform.system}.openreadout;
          meta.description = "Read raw lab-instrument files without vendor software";
        };
        default = openreadout;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.openreadout ];
          packages = with pkgs; [ cargo-nextest cargo-deny clippy rustfmt ];
        };
      });
    };
}
