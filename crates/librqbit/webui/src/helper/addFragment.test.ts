import { magnetHandlerUrl, parseAddFragment } from "./addFragment";
let n = 0;
const check = (c: boolean, m: string) => {
  n++;
  if (!c) throw new Error("FAIL: " + m);
};
// registerProtocolHandler substitutes the escaped link for %s.
check(
  parseAddFragment("#add=magnet%3A%3Fxt%3Durn%3Abtih%3Aabc%26dn%3DA%2BB") ===
    "magnet:?xt=urn:btih:abc&dn=A+B",
  "escaped magnet",
);
check(
  parseAddFragment("add=https%3A%2F%2Fx%2Fy.torrent") === "https://x/y.torrent",
  "http link without #",
);
check(parseAddFragment("#add=MAGNET%3A%3Fxt%3D1") === "MAGNET:?xt=1", "case-insensitive scheme");
check(parseAddFragment("#add=javascript%3Aalert(1)") === null, "javascript rejected");
check(parseAddFragment("#add=%2Fetc%2Fpasswd") === null, "paths rejected");
check(parseAddFragment("#other=1") === null, "other params ignored");
check(parseAddFragment("") === null && parseAddFragment("#") === null, "empty");
check(
  magnetHandlerUrl({ origin: "https://r.witherow.ca", pathname: "/web/" }) ===
    "https://r.witherow.ca/web/#add=%s",
  "handler url",
);
console.log(`addFragment: ${n} checks passed`);
