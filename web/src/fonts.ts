/** Geist and Geist Mono (SIL OFL), bundled so the page loads no font from anywhere else. */
import sans from "geist-fonts/geist-sans/Geist-Variable.woff2?url";
import mono from "geist-fonts/geist-mono/GeistMono-Variable.woff2?url";

for (const [family, url] of [["Geist", sans], ["Geist Mono", mono]] as const) {
  const face = new FontFace(family, `url(${url}) format("woff2")`, { weight: "100 900", display: "swap" });
  document.fonts.add(face);
  void face.load();
}
