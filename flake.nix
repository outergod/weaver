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
          stdenv.cc.cc
        ];

        RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";

        shellHook = ''
          export UV_PYTHON_DOWNLOADS=never
          export UV_PYTHON=$(command -v python)
          export LD_LIBRARY_PATH=${pkgs.stdenv.cc.cc.lib}/lib:$LD_LIBRARY_PATH

          # nixpkgs' stdenv pre-sets SOURCE_DATE_EPOCH=315532800
          # (1980-01-01) in every mkShell as a reproducible-build floor
          # for ZIP compatibility. `vergen` honors the value and would
          # stamp `weaver --version` with 1980, defeating L2 P11's
          # informative-timestamp intent. Clear it here so dev builds
          # see SystemTime::now(); `core/build.rs` also filters the
          # same sentinel defensively for non-nix dev shells.
          # Intentional reproducible-build callers (e.g. release CI)
          # should set SOURCE_DATE_EPOCH *after* entering the shell.
          unset SOURCE_DATE_EPOCH
        '';
      };
    };
}
