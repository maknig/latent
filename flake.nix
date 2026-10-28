{
  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  inputs.rust-overlay = {
    url = "github:oxalica/rust-overlay";
    inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; overlays = [ rust-overlay.overlays.default ]; };
      rust = pkgs.rust-bin.stable.latest.default.override {
        targets = [ "wasm32-unknown-unknown" ];
        extensions = [ "rust-src" "clippy" "rustfmt" ];
      };
      runtimeLibs = pkgs.lib.makeLibraryPath (with pkgs; [
        libxkbcommon libGL wayland vulkan-loader libx11 libxcursor libxi libxrandr
      ]);
      buildLibs = with pkgs; [
        pkg-config openssl libxkbcommon libGL wayland libx11 libxcursor libxi libxrandr
        gtk3 glib
      ];
      # Builds from the (read-only) flake source into a cache dir; no nix package because `ort`
      # downloads ONNX Runtime at build time, which the nix sandbox forbids.
      latentApp = pkgs.writeShellApplication {
        name = "latent";
        runtimeInputs = [ rust pkgs.gcc ] ++ buildLibs;
        text = ''
          export LD_LIBRARY_PATH="${runtimeLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
          export CARGO_TARGET_DIR="''${XDG_CACHE_HOME:-$HOME/.cache}/latent/target"
          exec cargo run --release --manifest-path ${self}/Cargo.toml -p latent -- "$@"
        '';
      };
    in {
      apps.${system}.default = { type = "app"; program = "${latentApp}/bin/latent"; };

      devShells.${system}.default = pkgs.mkShell {
        buildInputs = [ rust pkgs.trunk pkgs.wasm-bindgen-cli (pkgs.python3.withPackages (ps: [ ps.pillow ])) ] ++ buildLibs;
        LD_LIBRARY_PATH = runtimeLibs;
      };
    };
}
