{ lib, crate2nix, stdenv }:
let
  fakeCrates = {
    "pkg_id1" = "pkg_id1";
    "pkg_id2" = "pkg_id2";
    "pkg_id3" = "pkg_id3";
  };
  fakeDependencies = [
    {
      name = "id1";
      packageId = "pkg_id1";
    }
    {
      name = "optional_id2";
      packageId = "pkg_id2";
      optional = true;
    }
    {
      name = "id3";
      packageId = "pkg_id3";
      usesDefaultFeatures = false;
    }
  ];
  dependencyDerivations = features: dependencies:
    crate2nix.dependencyDerivations {
      buildByPackageId = p: fakeCrates.${p};
      target = crate2nix.makeDefaultTarget (stdenv.hostPlatform);
      inherit features dependencies;
    };
in
{

  testForDefaultAndIgnored = {
    expr = dependencyDerivations [ "default" "ignored" ] fakeDependencies;
    expected = [
      "pkg_id1"
      "pkg_id3"
    ];
  };

  testWithOptional = {
    expr = dependencyDerivations [ "default" "optional_id2" ] fakeDependencies;
    expected = [
      "pkg_id1"
      "pkg_id2"
      "pkg_id3"
    ];
  };

  testWithDepFeatures = {
    expr = dependencyDerivations
      [ "default" "id1/default" "id1/stuff" "id2/ignored_feature" "id3/feature1" ]
      fakeDependencies;
    expected = [
      "pkg_id1"
      "pkg_id3"
    ];
  };

}
