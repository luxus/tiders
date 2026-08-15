{
  lib,
  stdenv,
  rustPlatform,
  makeWrapper,
  mpv,
  libiconv,
}:

let
  inherit (lib.fileset) toSource unions;
  srcRoot = ../.;
  cargoToml = builtins.fromTOML (builtins.readFile (srcRoot + "/Cargo.toml"));
in
rustPlatform.buildRustPackage {
  pname = "tiders";
  version = cargoToml.workspace.package.version;

  src = toSource {
    root = srcRoot;
    fileset = unions [
      (srcRoot + "/Cargo.toml")
      (srcRoot + "/Cargo.lock")
      (srcRoot + "/crates")
      (srcRoot + "/rust-toolchain.toml")
    ];
  };

  cargoLock = {
    lockFile = srcRoot + "/Cargo.lock";
    outputHashes = {
      # Keep in sync with the tidlers git rev in Cargo.lock.
      "tidlers-0.5.0" = "sha256-yH0hMT3kGIhCe0DkLuME5yZCy7pTsfS9qcJ9YWijHUo=";
    };
  };

  strictDeps = true;
  nativeBuildInputs = [ makeWrapper ];
  buildInputs = lib.optionals stdenv.hostPlatform.isDarwin [ libiconv ];

  # Tests are unit-level (no network / no mpv).
  doCheck = true;

  postInstall = ''
    wrapProgram $out/bin/tiders --prefix PATH : ${lib.makeBinPath [ mpv ]}
  '';

  meta = {
    description = "A terminal (TUI + CLI) client for the TIDAL music streaming service";
    homepage = "https://github.com/luxus/tiders";
    changelog = "https://github.com/luxus/tiders/releases";
    license = lib.licenses.mit;
    mainProgram = "tiders";
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
  };
}
