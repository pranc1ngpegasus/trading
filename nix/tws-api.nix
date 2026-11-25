{
  lib,
  stdenv,
  fetchzip,
}:
let
  version = "1050.02";
in
stdenv.mkDerivation {
  pname = "tws-api";
  inherit version;

  src = fetchzip {
    url = "https://interactivebrokers.github.io/downloads/twsapi_macunix.${version}.zip";
    hash = "sha256-UjHZwPzZhbOnU3fLqlcaek8U2fzwcmwZvgbIxR9tJZ4=";
    stripRoot = false;
  };

  dontBuild = true;

  installPhase = ''
    runHook preInstall

    mkdir -p $out/share/tws-api
    cp -R $src/IBJts/. $out/share/tws-api/

    runHook postInstall
  '';

  meta = {
    description = "Interactive Brokers TWS API (C++, Java, and Python clients)";
    homepage = "https://interactivebrokers.github.io/";
    license = lib.licenses.gpl3Plus;
    platforms = lib.platforms.unix;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
