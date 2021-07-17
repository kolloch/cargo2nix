# crate2nix

crate2nix generates [nix](https://nixos.org/nix/) build files for [rust](https://www.rust-lang.org/) crates
using [cargo](https://crates.io/).


[![tests-nix-linux](https://github.com/kolloch/crate2nix/workflows/tests-nix-linux/badge.svg)](https://github.com/kolloch/crate2nix/actions?query=workflow%3Atests-nix-linux)
[![tests-nix-macos](https://github.com/kolloch/crate2nix/workflows/tests-nix-macos/badge.svg)](https://github.com/kolloch/crate2nix/actions?query=workflow%3Atests-nix-macos)
[![Crate](https://img.shields.io/crates/v/crate2nix.svg)](https://crates.io/crates/crate2nix)

**Same dependency tree as cargo**: It uses [cargo_metadata](https://github.com/oli-obk/cargo_metadata) to obtain the
dependency tree from cargo. Therefore, it will use the exact same library versions as cargo and respect any locked down
version in `Cargo.lock`.

**Smart caching**: It uses smart crate by crate caching so that nix rebuilds exactly the crates that need to be rebuilt.
Compare that to docker layers...

**Nix ecosystem goodness**: You can use all things that make the nix/NixOS ecosystem great, e.g. distributed/remote builds,
build minimal docker images, deploy your binary as a service to the cloud with [NixOps](https://nixos.org/nixops/), ...

**Out of the box support for libraries with non-rust dependencies**: It builds on top of the `buildRustCrate`
function from [NixOS](https://nixos.org/) so that native dependencies of
many rust libraries are already correctly fetched when needed. If your library with native dependencies is not yet
supported, you can customize `defaultCrateOverrides` / `crateOverrides`, see below.

**Easy to understand nix template**: The actual nix code is generated via `templates/build.nix.tera` so you can
fix/improve the nix code without knowing rust if all the data is already there.

Here is a simple example which uses all the defaults and will generate a `Cargo.nix` file:

```bash
# From the project directory.
crate2nix generate
```

Here is a more elaborate example that uses `<nixos-unstable>` as the default `pkgs` path (instead of `<nixpkgs>`) and specifies both the path
to the `Cargo.toml` file (`-f`) and the output (`-o`) file explicitly (usually not needed).

```bash
crate2nix generate \
    -n '<nixos-unstable>' \
    -f /some/project/dir/Cargo.toml \
    -o /some/project/dir/Cargo.nix
```

Use `crate2nix help` to show all commands and options.

## Installation

If you are not running, install a recent version of nix by running `curl https://nixos.org/nix/install | sh` or following
the instructions on [https://nixos.org/nix/](https://nixos.org/nix/).

Then

```bash
# Install the stable version to your user env (with shell completions):
nix-env -i -f https://github.com/kolloch/crate2nix/tarball/0.10.0
```

NOTE: You can use [eigenvalue.cachix.org](https://eigenvalue.cachix.org/) to
get prebuilt binaries for linux.

### Development Version (master)

Similarly, you can install crate2nix by

```bash
# Install the unstable version to your user env (with shell completions):
nix-env -i -f https://github.com/kolloch/crate2nix/tarball/master
```

If you want to tweak crate2nix, clone the repository and then

```bash
cd crate2nix
# to run crate2nix without installing it
./crate2nix.sh
# or to install it in your user environment
nix-env -i -f .
```

## Generating build files

The `crate2nix generate` command generates a nix file. You can specify the output file with `-o`. E.g.

```bash
crate2nix generate
```

generates Cargo.nix from the Cargo.lock in the current directory.

Look at the [./crate2nix/Cargo.nix](./crate2nix/Cargo.nix) file of this project for a non-trivial example. (How meta!)

## Using build files (single binaries)

If your `Cargo.nix` was generated for a single binary crate (i.e. workspace) then the derivation that builds your binary
can be accessed via the `rootCrate.build` attribute. Use this command to build it and make the result available in the result
directory:

```bash
your_crate_name="super_duper"
nix build -f Cargo.nix rootCrate.build
./result/bin/${your_crate_name}
```

Within a nix file (e.g. your manually written `default.nix`), you can access the
derivation like this:

```nix
let cargo_nix = callPackage ./Cargo.nix {};
in cargo_nix.rootCrate.build
```

## Using build files (workspaces)

If your `Cargo.nix` was generated for a workspace (i.e. not a single binary) then the derivation that builds your binary
CANNOT be accessed via the `rootCrate` attribute. There is no single root crate.

Instead, you can conveniently access the derivations of all your workspace members through the `workspaceMembers`
attribute. Use this command to build one of the workspace members and make the result available in the result
directory:

```bash
your_crate_name="super_duper"
nix build -f Cargo.nix workspaceMembers.${your_crate_name}.build
./result/bin/${your_crate_name}
```

Within a nix file (e.g. your manually written `default.nix`), you can access the
derivation like this:

```nix
let cargo_nix = callPackage ./Cargo.nix {};
in cargo_nix.workspaceMembers."${your_crate_name}".build
```

## Choosing a Rust Version

If you want to compile your code with a different rust version than is currently
available in your nixpkgs, you can use the awesome overlays of the
[nixpkgs-mozilla](https://github.com/mozilla/nixpkgs-mozilla).

For managing the git dependencies, I recommend
[niv](https://github.com/nmattia/niv). To create an initial "sources.nix" in the
nix directory run:

```shell
niv init
niv add mozilla/nixpkgs-mozilla
```

Later on, you can update your dependencies with `niv update`.

To pin your nixpkgs with the appropriate overlays, place a `nixpkgs.nix` file into
you `nix` directory that was created by `niv` (if necessary):

```nix
let
  # Manage this with https://github.com/nmattia/niv
  # or define { nixpkgs = ...; nixpkgs-mozilla = ...; }
  # yourself.
  sources = import ./sources.nix;

  rustChannelsOverlay = import "${sources.nixpkgs-mozilla}/rust-overlay.nix";
  # Useful if you also want to provide that in a nix-shell since some rust tools depend
  # on that.
  rustChannelsSrcOverlay = import "${sources.nixpkgs-mozilla}/rust-src-overlay.nix";

in import sources.nixpkgs {
    overlays = [
      rustChannelsOverlay
      rustChannelsSrcOverlay
      (self: super: {
        # Replace "latest.rustChannels.stable" with the version of the rust tools that
        # you would like. Look at the documentation of nixpkgs-mozilla for examples.
        #
        # NOTE: "rust" instead of "rustc" is not a typo: It will include more than needed
        # but also the much needed "rust-std".
        rustc = super.latest.rustChannels.stable.rust;
        inherit (super.latest.rustChannels.stable) cargo rust rust-fmt rust-std clippy;
      })
    ];
  }
```

In your `default.nix` or other nix files, you can use the following to refer to
that pinned `pkgs` with the rust version of your choice:

```nix
{ pkgs ? import ./nix/nixpkgs.nix }:

let cargoNix = pkgs.callPackage ./Cargo.nix {};
in cargoNix.rootCrate.build
```

## Dynamic feature resolution

The enabled features for a crate now are resolved at build time! That means you can easily override them:

1. There is a "rootFeatures" argument to the generated build file which you can override when calling
   it from the command line:

      ```bash
      nix build -f ....nix --arg rootFeatures '["default" "other"]' rootCrate.build
      ```

2. Or when importing the build file with "callPackage":

      ```nix
      let cargo_nix = callPackage ./Cargo.nix { rootFeatures = ["default" "other"]; };
          crate2nix = cargo_nix.rootCrate.build;
      in ...
      ```

3. Or by overriding them on the rootCrate or workspaceMembers:

      ```nix
      let cargo_nix = callPackage ./Cargo.nix {};
          crate2nix = cargo_nix.rootCrate.build.override { features = ["default" "other"]; };
      in ...
      ```
Note that only dependencies for the default features are included in the build.
If you want full flexibility, you can use `crate2nix generate --all-features` to
generate the most general build file. If you want to strip down the generated
build file, you may want to use `crate2nix generate --no-default-features
--features "feature1 feature2"`.

## Patching crate derivations with `crateOverrides`

NixOS comes with
[`defaultCrateOverrides`](https://github.com/NixOS/nixpkgs/blob/master/pkgs/build-support/rust/default-crate-overrides.nix)
which specifies mostly some additional native `buildInputs` for various popular
crates. If you are using a rust crate with native dependencies which is not yet
covered, you can add additional `buildInputs` with the `crateOverride` parameter
(similar to `features`):

```nix
let
  customBuildRustCrateForPkgs = pkgs: pkgs.buildRustCrate.override {
    defaultCrateOverrides = pkgs.defaultCrateOverrides // {
      funky-things = attrs: {
        buildInputs = [ pkgs.openssl ];
      };
    };
  };
  generatedBuild = callPackage ./crate2nix/Cargo.nix {
    buildRustCrateForPkgs = customBuildRustCrateForPkgs;
  };
in generatedBuild.rootCrate.build
```

Or obviously you can use the power of nix to add a dependency conditionally:

```nix
let
  customBuildRustCrateForPkgs = pkgs: pkgs.buildRustCrate.override {
    defaultCrateOverrides = pkgs.defaultCrateOverrides // {
      cssparser-macros = attrs: {
        buildInputs =
          lib.optionals
            pkgs.stdenv.isDarwin
            [ pkgs.darwin.apple_sdk.frameworks.Security ];
      };
    };
  };
  generatedBuild = callPackage ./crate2nix/Cargo.nix {
    buildRustCrateForPkgs = customBuildRustCrateForPkgs;
  };
in generatedBuild.rootCrate.build
```

`crateOverrides` are not restricted to buildInputs however. You should also be
able to add patches and the like! (I didn't try that, though.)

`crateOverrides` are a feature of the underlying `buildRustCrate` support in
NixOS that crate2nix uses.

## Running rust tests

There is some experimental support for running tests of your rust crates. All of
the crates in the workspace will have their tests executed. When enabling test
execution (`runTests = true;`), failing tests will make the whole build fail
unless you explicitly disable this via test hooks: see the section below.

```nix
      let cargo_nix = callPackage ./Cargo.nix {};
          crate2nix = cargo_nix.rootCrate.build.override {
	    runTests = true;
	    testInputs = [ pkgs.cowsay ];
	  };
      in ...
```

`testInputs` is optional and allows passing inputs to the test execution that
should be in scope. Defaults to an empty list and is ignored when `runTests`
equals `false`.

### Custom pre/post test hooks

Want to customize your test execution? Use the `testPreRun` and `testPostRun`
crate attributes(next to `runTests` in the example above). `crate2nix` executes
the bash snippets in `testPreRun` and `testPostRun` directly before and after
the actual test command, and in the same shell. Some example use-cases include:

* Setting some environment variable that is needed for the test.

* Setting (and then unsetting) the bash `set +e` option to not fail the
  derivation build even if a test fails. This is quite useful if your tests are
  not flaky and you want to cache failures.

## FAQ

## Known Restrictions

`crate2nix` makes heavy use of `buildRustCrate` in `nixpkgs`. So we potentially depend on features in a recent version
of `nixpkgs`. Check [nix/sources.json](https://github.com/kolloch/crate2nix/blob/master/nix/sources.json) for the version
of nixpkgs that `crate2nix` is tested against.

If you feel limited by these restrictions, please do not hesitate to file an issue! That
gives me a feeling of what is worth working on.

* There is only experimental support for running tests ~~Before 0.7.x: No
  support for building and running tests, see [nixpkgs, issue
  59177](https://github.com/NixOS/nixpkgs/issues/59177).~~
* Target-specific features do not work automatically, see
  [#129](https://github.com/kolloch/crate2nix/issues/129). You should be able to
  enable the required features manually, however.
* A crate will only have access to its own source directory during build time
  and not e.g. to other directories in the same workspace. See [crate2nix, issue
  17](https://github.com/kolloch/crate2nix/issues/17). I used to consider this
  "works as intended" but now I think that we should use the "workspaceMember"
  argument of buildRustCrate for this.
* It does translates target strings to nix expressions. The support should be
  reasonable but probably not complete - please let me know if you hit problems.
  ~~Before 0.2.x: Filters all dependencies for the *hard-coded "Linux x86_64"
  target platform*. Again, it should be quite easy to support more platforms. To
  do so completely and at build time (vs build generation time) might be more
  involved.~~

Former restrictions, now supported:

* ~~Before 0.8.x: Since cargo exposes local paths in package IDs, the generated
  build file also contain them as part of an "opaque" ID. They are not
  interpreted as paths but maybe you do not want to expose local paths in
  there...~~ The full opaque package ID will only be used if you have the same
  package with the same version multiple times. That should be very rare.
* ~~Before 0.6.x: [Renamed
  crates](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#renaming-dependencies-in-cargotoml)
  with an explicit `package` name don't work yet.~~
* Git sources are now also supported. Starting with 0.7 sub modules also work.
  Finding crates in arbitrary sub directories of git sources (which cargo
  supports!)is not supported, see #53.
* ~~Before 0.4.x: Only *default crate features* are supported. It should be easy
  to support a different feature set at build generation time since we can
  simply pass this set to `cargo metadata`. Feature selection during build time
  is out of scope for now.~~

## Feedback: What is needed for a 1.0 release?

I would really appreciate your thoughts. Please add comments to issue
[#8](https://github.com/kolloch/crate2nix/issues/8).

## Runtime Dependencies

crate2nix use `cargo metadata` / `nix-prefetch-url` at runtime so they need to be in the PATH. The default.nix
adds the built-time nix/cargo binaries as fallback to the path.

Currently, crate2nix is only tested with nixpkgs-unstable since it depends on
some new features and bug fixes.

## Project Overview / Terminology

If you want to hack on this, it is useful to know that build file generation is broken up into multiple phases:

1. **cargo metadata**: Calling `cargo metadata` via the `cargo_metadata` crate.
2. **indexing metadata**: Indexing the metadata by package ID to enable easy joining of "Node" and "Package"
  information, resulting in `metadata::IndexedMetadata`.
3. **resolving**: Using the indexed metadata to actually resolve the dependencies and join all needed build information
  into `resolve::CrateDerivation`.
4. **pre-fetching**: Pre-fetching crates.io packages to determine their sha256, see `prefetch` module.
5. **rendering**: Rendering the data via the `build.nix.tera` template, see `render` module.

## Related Projects

* [carnix](https://nest.pijul.com/pmeunier/carnix:master) is already
  widely used in NixOS itself, yet it failed to
  generate correct builds for my rust projects. After some attempts to fix that, I gave up. That said, big kudos for
  all the work on buildRustCrate and showing the way!
* [naersk](https://github.com/nmattia/naersk/) uses cargo to drive the
  entire build. It builds all dependencies in one derivation and the crate itself in another. Since it relies on hashes from the Cargo.lock
  file, I don't know how it handles git dependencies with sub modules.
* [tenx-tech/cargo2nix](https://github.com/tenx-tech/cargo2nix): I
  haven't used it so take it with a grain of salt but I think
    * it uses its own build logic instead of `buildRustCrate` but
      still builds each crate in its own derivation.
    * it has some support for cross building (which is quite weak in
      crate2nix).
* [cargo-raze](https://github.com/google/cargo-raze) generates `BUILD`
  files for bazel.

## Contributions

Contributions in the form of documentation and bug fixes are highly welcome.
Please start a discussion with me before working on larger features.

I'd really appreciate tests for all new features. Please run `./run_tests.sh`
before submitting a pull request.

Feature ideas are also welcome -- just know that this is a pure hobby side
project and I will not allocate a lot of bandwidth to this. Therefore, important
bug fixes are always prioritised.

By submitting a pull request, you agree to license your changes via all the
current licenses of the project.

### Regenerating Cargo.nix files for tests

If you change `crate2nix` such that it will produce a different output, you may
need to regenerate some of the Cargo.toml files. Not all `Cargo.toml` files can
be generated during test time because crate2nix needs to vendor the dependencies
for this to work and support for git submodules is not working yet, see
[#101](https://github.com/kolloch/crate2nix/issues/101).

`regenerate_cargo_nix.sh` should do what you want and is run as part of
`run_tests.sh` (see below).

### Running tests

`run_tests.sh` will regenerate build files AND run cargo test for you. It will
call out to nix to build the sample projects -- so a considerable number of
dependencies will be fetched and built. Consecutive runs are much faster.

### Hacking on `buildRustCrate` in nixpkgs

Since `crate2nix` heavily depends on `buildRustCrate` in `nixpkgs`, it makes
sense to hack on them together.

To be able to provide pull requests, you probably want to fork
[nixpkgs](https://github.com/NixOS/nixpkgs) first. Once you have done that,
clone that fork into some local directory (separate from crate2nix).

#### Overriding nixpkgs for everything

Now you can run the integration tests of `crate2nix` against that version of
nixpkgs. Let's assume you are in the `crate2nix` project directory and you
cloned `nixpkgs` to a sibling directory:

```shell
nix-build --arg nixpkgs ../nixpkgs -o ./crate2nix/target/nix-results ./tests.nix
```

Or run just an individual test (in this example "bin_with_lib_dep"):

```shell
nix-build --arg nixpkgs ../nixpkgs \
  -o ./crate2nix/target/nix-results ./tests.nix -A bin_with_lib_dep
```

(The "-o" argument is just to avoid a lot of top-level result directories.)

#### Overriding nixpkgs for buildTests

The problems is that this method will rebuild everything with your nixpkgs,
including `crate2nix` itself. That can become severely annoying if you want
to iterate on one special test case.

If you are fixing an issue in `buildRustCrate` that you can reproduce with a
`buildTest` in `tests.nix`, then there is a much better way.

```shell
nix-build --arg buildTestNixpkgs ../nixpkgs \
  -o ./crate2nix/target/nix-results ./tests.nix -A bin_with_lib_dep
```

Notice `--arg buildTestNixpkgs` instead of `--arg nixpkgs`. That will not
rebuild `crate2nix` itself with your nixpkgs but it will use `buildRustCrate`
from your `nixpkgs` for all `buildTests`.
