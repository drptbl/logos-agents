{
  description = "logos-agents as a Basecamp module";
  inputs.logos-module-builder.url = "github:logos-co/logos-module-builder/0.3.3";
  outputs = inputs@{ self, logos-module-builder, ... }:
    let
      nixpkgs = logos-module-builder.inputs.nixpkgs;
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems f;
    in {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          # The builder copies only the crate folder, so put the repo's library
          # inside it, where module/Cargo.toml's path dependency expects it.
          # logos-mcp comes from git, pinned by module/Cargo.lock.
          src = pkgs.runCommand "logos-agents-module-src" { } ''
            cp -r ${./.} $out
            chmod -R u+w $out
            rm -rf $out/module/vendor
            mkdir -p $out/module/vendor/logos-agents
            cp -r ${../src} $out/module/vendor/logos-agents/src
            cp ${../Cargo.toml} $out/module/vendor/logos-agents/Cargo.toml
            cp ${../README.md} $out/module/vendor/logos-agents/README.md
          '';
        in
        (logos-module-builder.lib.mkLogosModule {
          inherit src;
          configFile = ./metadata.json;
          flakeInputs = inputs;
        }).packages.${system});
    };
}
