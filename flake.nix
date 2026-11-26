{
  description = "Turbine: a retro DOS-style, AI-first code editor and IDE";

  # Pinned in flake.lock to the running system's nixpkgs revision; bump together.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs:
        let
          cargoDeps = pkgs.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; };

          # Build in a persistent target directory (/var/tmp/cargo-nix, exposed
          # to the sandbox by the system flake) so rebuilds only recompile what
          # changed. Falls back to a throwaway directory elsewhere.
          persistentCargo = name: cargoArgs: ''
            runHook preBuild
            workDir=$TMPDIR/cargo-nix
            if [ -w /var/tmp/cargo-nix ]; then
              workDir=/var/tmp/cargo-nix/${name}
            fi
            sed -i "s|^directory = .*|directory = \"${cargoDeps}\"|" "$NIX_BUILD_TOP/.cargo/config.toml"
            (
              umask 000
              mkdir -p "$workDir/src" "$workDir/target"
              touch "$workDir"
              rsync -rc --no-times --no-perms --chmod=a+rwX --delete ./ "$workDir/src/"
              export NIX_ENFORCE_PURITY=0
              cd "$workDir/src"
              CARGO_HOME="$NIX_BUILD_TOP/.cargo" CARGO_TARGET_DIR="$workDir/target" \
                cargo build ${cargoArgs} --offline --locked -j "$NIX_BUILD_CORES"
              find "$workDir/target" -mindepth 1 -user "$(id -u)" ! -perm -o+w -exec chmod a+rwX {} + 2>/dev/null || true
            )
            ln -sfn "$workDir/target" target
            runHook postBuild
          '';
        in
        rec {
          turbine = pkgs.rustPlatform.buildRustPackage {
            pname = "turbine";
            version = "0.1.0";
            src = pkgs.lib.cleanSource ./.;
            inherit cargoDeps;
            nativeBuildInputs = [ pkgs.rsync ];
            buildPhase = persistentCargo "turbine" "--release -p turbine";
            installPhase = ''
              runHook preInstall
              install -Dm755 target/release/turbine $out/bin/turbine
              runHook postInstall
            '';
            doCheck = false;
            meta = {
              description = "A retro DOS-style, AI-first code editor and IDE";
              license = pkgs.lib.licenses.mit;
              mainProgram = "turbine";
            };
          };
          default = turbine;
        });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc rustfmt clippy rust-analyzer ];
        };
      });
    };
}
