{ sources ? import ./sources.nix
, pkgs ? import sources.nixpkgs { }
  # Use last pinned crate2nix packages to build the test runner
  # so that it works even if we have broken stuff!
, tools ? pkgs.callPackage "${sources.crate2nix}/tools.nix" { }
}:

{
  dev = {

    inherit (pkgs)
      cargo clippy rustc rustfmt
      binutils
      nixpkgs-fmt jq
      niv
      nix
      git
      utillinux
      cacert
      nix-prefetch-git
      ;

    nixTest =
      let
        cargoNix = tools.appliedCargoNix rec {
          name = "nix-test-runner";
          src = sources."${name}";
        };
      in
      cargoNix.rootCrate.build;

    cargoRelease =
      let
        cargoNixSource = tools.generatedCargoNix rec {
          name = "cargo-release";
          src = sources."${name}";
        };
        buildRustCrateForPkgs = pkgs: pkgs.buildRustCrate.override {
          defaultCrateOverrides = pkgs.defaultCrateOverrides // {
            cargo-release = { buildInputs ? [ ], ... }: {
              buildInputs = buildInputs ++ [ pkgs.openssl ];
            };
          };
        };
        cargoNix = import cargoNixSource { inherit pkgs buildRustCrateForPkgs; };
      in
      cargoNix.rootCrate.build;
  };
}
