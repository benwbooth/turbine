{
  description = "Turbine: a retro DOS-style, AI-first code editor and IDE";

  # Pinned in flake.lock to the running system's nixpkgs revision; bump together.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      # Qt for the desktop app (turbine-app, cxx-qt): one merged tree with qmake.
      qtFor = pkgs: with pkgs.qt6; [ qtbase qtdeclarative qtsvg qtwayland ];
      qtEnvFor = pkgs: pkgs.qt6.env "qt-turbine-${pkgs.qt6.qtbase.version}" ((builtins.tail (qtFor pkgs)) ++ [ pkgs.libGL ]);
    in
    {
      packages = forAll (pkgs:
        let
          cargoDeps = pkgs.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; };

          # Ghostty's terminal engine (libghostty-vt-sys builds it with Zig):
          # the commit its build.rs pins and the Zig packages it needs,
          # fetched ahead of time because builds are offline (as Tessera).
          ghosttySrc = pkgs.fetchFromGitHub {
            owner = "ghostty-org";
            repo = "ghostty";
            rev = "a887df42c56f6de86c0fe6da9c4eeca37931e083";
            hash = "sha256-1Zz65SCk3rkJ9+Q0MmyNOTNiDSLBRIHRd3IvFM4iNXw=";
          };
          zigDeps = pkgs.stdenvNoCC.mkDerivation {
            name = "ghostty-zig-deps";
            src = ghosttySrc;
            nativeBuildInputs = [ pkgs.zig_0_15 ];
            dontConfigure = true;
            dontFixup = true;
            buildPhase = ''
              export ZIG_GLOBAL_CACHE_DIR=$TMPDIR/zig
              zig build --fetch=all --global-cache-dir $TMPDIR/zig
            '';
            installPhase = "cp -r $TMPDIR/zig/p $out";
            outputHashMode = "recursive";
            outputHashAlgo = "sha256";
            outputHash = "sha256-PnM+hZIlLyQwK8vJgd/Bhjt1lNIz06T8FahwliRmMrY=";
          };

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
              # Zig writes into Ghostty's tree: a writable copy at a fixed path,
              # so cargo doesn't rebuild the engine every time.
              rsync -rc --no-times --no-perms --chmod=a+rwX ${ghosttySrc}/ "$workDir/ghostty/"
              export GHOSTTY_SOURCE_DIR="$workDir/ghostty"
              export GHOSTTY_ZIG_SYSTEM_DIR="${zigDeps}"
              export ZIG_GLOBAL_CACHE_DIR="$workDir/zig-cache"
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
            nativeBuildInputs = [ pkgs.rsync pkgs.zig_0_15 ];
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

      devShells = forAll (pkgs:
        let qtEnv = qtEnvFor pkgs; in
        {
          default = pkgs.mkShell {
            # Zig 0.15 and git: libghostty-vt-sys builds Ghostty's terminal engine.
            # Qt (qmake) for turbine-app.
            packages = with pkgs; [ cargo rustc rustfmt clippy rust-analyzer zig_0_15 git pkg-config qtEnv libGL ];
            QMAKE = "${qtEnv}/bin/qmake";
            # Link with this shell's compiler: a system clang links against the
            # system glibc and the binary can't load this nixpkgs' Qt.
            "CARGO_TARGET_${pkgs.stdenv.hostPlatform.rust.cargoEnvVarTarget}_LINKER" = "cc";
            shellHook = ''
              export NIX_LDFLAGS="''${NIX_LDFLAGS//"-rpath $out/lib"/}"
            '';
          };
        });
    };
}
