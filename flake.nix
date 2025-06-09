{
  description = "A very basic flake";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      flake-utils,
      ...
    }:
    let
      nativeDeps = pkgs: [
        pkgs.pkg-config
      ];

      deps = pkgs: [
        pkgs.alsa-lib
      ];

      mkCrabTap =
        pkgs:
        let
          rustPlatform = pkgs.makeRustPlatform {
            cargo = pkgs.rust-bin.stable.latest.default;
            rustc = pkgs.rust-bin.stable.latest.default;
          };
        in
        rustPlatform.buildRustPackage {
          pname = "crabtap";
          version = "0.1";
          src = ./.;

          nativeBuildInputs = nativeDeps pkgs;
          buildInputs = deps pkgs;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };
        };

      systemOutputs = flake-utils.lib.eachDefaultSystem (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };

        in
        {
          packages.default = mkCrabTap pkgs;

          devShell = pkgs.mkShell {
            nativeBuildInputs = nativeDeps pkgs;
            buildInputs = deps pkgs;
            packages = [
              pkgs.rust-bin.stable.latest.default
            ];
          };
        }
      );
    in
    systemOutputs
    // {
      overlays.default = final: prev: {
        crabtap = mkCrabTap prev.pkgs;
      };
    };
}
