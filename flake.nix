{
  description = "BL706 chiptool PAC and Embassy RAM bring-up";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
  };
  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = nixpkgs.lib.genAttrs systems;
      env = system:
        let
          pkgs = import nixpkgs { inherit system; overlays = [ rust-overlay.overlays.default ]; };
          rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          platform = pkgs.makeRustPlatform { cargo = rust; rustc = rust; };
          chiptool = platform.buildRustPackage {
            pname = "chiptool";
            version = "bcf538a";
            src = pkgs.fetchFromGitHub {
              owner = "embassy-rs";
              repo = "chiptool";
              rev = "bcf538a2e7b8584ae874ee9ab72efb1576fc6152";
              hash = "sha256-4sboVcky2VFwiP3f6/DkfI49hCatgckD3w577uMe5K4=";
            };
            cargoLock = {
              lockFile = ./tools/chiptool.Cargo.lock;
              outputHashes."svd-parser-0.14.9" = "sha256-z7vF93ihmCCM8Fm9DO4FgvmN22CXyFaJjCrV+aMpS7c=";
            };
            doCheck = false;
          };
          firmware = platform.buildRustPackage {
            pname = "pinecil-v2-ram-bringup";
            version = "0.1.0";
            auditable = false; # cargo-auditable injects host -Wl flags into rust-lld.
            src = self;
            cargoLock.lockFile = ./Cargo.lock;
            buildPhase = ''
              runHook preBuild
              cargo firmware --locked --offline
              runHook postBuild
            '';
            installPhase = ''
              mkdir -p $out/bin
              for example in heartbeat display fusb302 i2c_latency interrupt_executor gpio_irq analog sampling system flash_probe; do
                cp target/riscv32imc-unknown-none-elf/release/$example $out/bin/$example.elf
              done
            '';
            doCheck = false;
            dontFixup = true;
          };
          pacCheck = pkgs.runCommand "bl706-pac-regeneration" {
            nativeBuildInputs = [ rust chiptool pkgs.uv pkgs.python3 ];
          } ''
            cp -r ${self} source
            chmod -R u+w source
            cd source
            export UV_PYTHON=${pkgs.python3}/bin/python3
            export UV_PYTHON_DOWNLOADS=never
            export UV_OFFLINE=1
            export UV_CACHE_DIR=$TMPDIR/uv-cache
            export SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt
            cp crates/bl706-pac/src/lib.rs ../expected.rs
            bash tools/generate-pac.sh
            cmp ../expected.rs crates/bl706-pac/src/lib.rs
            uv run --no-project tools/check_svd.py
            touch $out
          '';
        in { inherit pkgs rust chiptool firmware pacCheck; };
    in {
      packages = forAll (system: { inherit (env system) chiptool firmware; });
      checks = forAll (system: { pac = (env system).pacCheck; firmware = (env system).firmware; });
      devShells = forAll (system:
        let e = env system; in { default = e.pkgs.mkShell {
          packages = [ e.rust e.chiptool e.pkgs.uv e.pkgs.git e.pkgs.llvm e.pkgs.openocd ];
          UV_PYTHON_DOWNLOADS = "never";
          UV_PYTHON = "${e.pkgs.python3}/bin/python3";
        }; });
    };
}
