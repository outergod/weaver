{
  description = "Project dev shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs, ... }:
    let
      system = "x86_64-linux"; # change as needed
      pkgs = import nixpkgs { inherit system; config.allowUnfree = true; };
    in {
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          yaml-language-server
          cargo
          rustc
          rust-analyzer
          rustfmt
          clippy
          pkg-config
          git
          uv
          python314
          jq
          envsubst
          multimarkdown
        ];

        RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
      };
    };
}
