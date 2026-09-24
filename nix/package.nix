{ lib
, stdenv
, fetchurl
}:

stdenv.mkDerivation rec {
  pname = "rqbit";
  version = "9.0.1";

  src = fetchurl {
    url = "https://github.com/ikatson/rqbit/releases/download/v${version}/rqbit-linux-amd64";
    sha256 = "0da3vbiqq74pw3wwsyrkvxn3h3c598pbkaifr6r1pff7yhijrvc2";
  };

  dontUnpack = true;

  installPhase = ''
    mkdir -p $out/bin
    install -m755 $src $out/bin/rqbit
  '';

  meta = {
    description = "BitTorrent client with an HTTP API and web UI";
    mainProgram = "rqbit";
    license = lib.licenses.asl20;
    platforms = [ "x86_64-linux" ];
  };
}
