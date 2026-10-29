{
  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  inputs.rust-overlay = {
    url = "github:oxalica/rust-overlay";
    inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      # No x86_64-darwin: `ort` ships no prebuilt ONNX Runtime for it.
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);

      perSystem = system:
        let
          pkgs = import nixpkgs { inherit system; overlays = [ rust-overlay.overlays.default ]; };
          inherit (pkgs.stdenv.hostPlatform) isLinux;
          rust = pkgs.rust-bin.stable.latest.default.override {
            targets = [ "wasm32-unknown-unknown" ];
            extensions = [ "rust-src" "clippy" "rustfmt" ];
          };
          # Linux windowing/GL libs; on macOS eframe uses the system frameworks from the stdenv SDK.
          linuxLibs = with pkgs; [
            libxkbcommon
            libGL
            wayland
            vulkan-loader
            libx11
            libxcursor
            libxi
            libxrandr
          ];
          runtimeLibs = pkgs.lib.makeLibraryPath linuxLibs;
          buildLibs = [ pkgs.pkg-config pkgs.openssl ]
            ++ pkgs.lib.optionals isLinux (linuxLibs ++ [ pkgs.gtk3 pkgs.glib ]);
          # Builds from the (read-only) flake source into a cache dir; no nix package because `ort`
          # downloads ONNX Runtime at build time, which the nix sandbox forbids.
          latentApp = pkgs.writeShellApplication {
            name = "latent";
            runtimeInputs = [ rust pkgs.stdenv.cc ] ++ buildLibs;
            text = ''
              export PKG_CONFIG_PATH="${pkgs.lib.makeSearchPath "lib/pkgconfig" (map pkgs.lib.getDev buildLibs)}:${pkgs.lib.makeSearchPath "share/pkgconfig" (map pkgs.lib.getDev buildLibs)}"
              ${pkgs.lib.optionalString isLinux ''export LD_LIBRARY_PATH="${runtimeLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"''}
              export CARGO_TARGET_DIR="''${XDG_CACHE_HOME:-$HOME/.cache}/latent/target"
              exec cargo run --release --manifest-path ${self}/Cargo.toml -p latent -- "$@"
            '';
          };
        in
        {
          app = { type = "app"; program = "${latentApp}/bin/latent"; };
          devShell = pkgs.mkShell ({
            buildInputs = [ rust pkgs.trunk pkgs.wasm-bindgen-cli (pkgs.python3.withPackages (ps: [ ps.pillow ])) ] ++ buildLibs;
          } // pkgs.lib.optionalAttrs isLinux { LD_LIBRARY_PATH = runtimeLibs; });
        };
    in
    {
      apps = forAllSystems (system: { default = (perSystem system).app; });
      devShells = forAllSystems (system: { default = (perSystem system).devShell; });
    };
}
