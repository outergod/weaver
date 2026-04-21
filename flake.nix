{
  description = "Project dev shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # `fenix` provides a Rust toolchain that honors `rust-toolchain.toml`
    # the same way `rustup` does. Keeping the nix dev shell and CI on
    # identical rustc/clippy/rustfmt versions is what lets pre-commit
    # `cargo lint` catch everything CI's `clippy --all-targets -D
    # warnings` catches — no more local-passes-but-CI-fails on lints
    # whose default level changed between Rust versions.
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, fenix, ... }:
    let
      system = "x86_64-linux"; # change as needed
      pkgs = import nixpkgs { inherit system; config.allowUnfree = true; };

      # Read `rust-toolchain.toml` directly so the channel + components
      # listed there drive both this dev shell and CI's rustup.
      #
      # Bump discipline: when `rust-toolchain.toml`'s `channel` changes,
      # swap the `sha256` below with an all-zero placeholder
      # (`sha256-AAAAAA...=`); `nix develop` will report the correct
      # hash in the resulting error and that goes back here.
      rustToolchain = fenix.packages.${system}.fromToolchainFile {
        file = ./rust-toolchain.toml;
        sha256 = "sha256-qqF33vNuAdU5vua96VKVIwuc43j4EFeEXbjQ6+l4mO4=";
      };

      # Dev-friendly rust-analyzer tracks nightly, so pull it from the
      # fenix `rust-analyzer` output rather than bundling it into the
      # toolchain. Keeps the IDE current without forcing nightly
      # rustc/clippy into builds.
      rustAnalyzer = fenix.packages.${system}.rust-analyzer;
    in {
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          yaml-language-server
          pkg-config
          git
          uv
          python314
          jq
          envsubst
          stdenv.cc.cc
        ] ++ [
          rustToolchain
          rustAnalyzer
        ];

        # Point rust-analyzer at the pinned toolchain's std sources.
        RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";

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
