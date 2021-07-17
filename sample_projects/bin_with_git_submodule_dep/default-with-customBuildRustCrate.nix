{ pkgs ? import ../../nix/nixpkgs.nix { config = { }; }
, generatedCargoNix ? ./Cargo.nix
}:
let
  customBuildRustCrate = pkgs: pkgs.buildRustCrate.override {
    defaultCrateOverrides = pkgs.defaultCrateOverrides // {
      librocksdb-sys = attrs: with pkgs; {
        src = attrs.src + "/librocksdb-sys";
        buildInputs = [ clang rocksdb ];
        LIBCLANG_PATH = "${clang.cc.lib}/lib";
        ROCKSDB_LIB_DIR = "${rocksdb}/lib/";
      };
    };
  };
  basePackage = pkgs.callPackage generatedCargoNix { buildRustCrateForPkgs = customBuildRustCrate; };
  submodulePackage = basePackage.rootCrate.build;
in
submodulePackage
