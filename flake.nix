{
  description = "A very basic flake";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-23.11";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }: 
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        nativeDeps = with pkgs; [ pkg-config ];
        deps = with pkgs; [ alsa-lib ];

        rustPlatform = pkgs.makeRustPlatform {
          cargo = pkgs.rust-bin.stable.latest.default;
          rustc = pkgs.rust-bin.stable.latest.default;
        };

      in
      {
        packages = {
          default = rustPlatform.buildRustPackage {
            pname = "crabtap";
            version = "0.1";
            src = ./.;

            nativeBuildInputs = nativeDeps;
            buildInputs = deps;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };
          };
        };

        devShell = pkgs.mkShell {
          nativeBuildInputs = nativeDeps;
          buildInputs = deps;
          packages = with pkgs; [
            pkgs.rust-bin.stable.latest.default
          ];
        };
      }
    );
}
