/**
 * The page's icons: diffr's own glyphs, plus a few from @pierre/diffs' sprite (Apache-2.0) for the
 * layout toggle and the GitHub link, so they match the icons inside each file.
 */
import { SVGSpriteSheet } from "pierre-diffs-sprite";

const svg = (body: string, size = 16) =>
  `<svg viewBox="0 0 16 16" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
const sprite = (id: string, size = 16) =>
  `<svg viewBox="0 0 16 16" width="${size}" height="${size}" fill="currentColor" aria-hidden="true"><use href="#diffs-icon-${id}"></use></svg>`;

/** diffr's mark: three lines of code, the middle one moved, cut out of a rounded tile. */
export const logo = (size = 24) =>
  `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="currentColor" fill-rule="evenodd" role="img" aria-label="diffr">`
  + `<path d="M7 0h10c5 0 7 2 7 7v10c0 5-2 7-7 7H7c-5 0-7-2-7-7V7c0-5 2-7 7-7Z`
  + `M5.75 6a1 1 0 0 0 0 2h7.5a1 1 0 0 0 0-2zM9.75 11a1 1 0 0 0 0 2h8.5a1 1 0 0 0 0-2zM5.75 16a1 1 0 0 0 0 2h5.5a1 1 0 0 0 0-2z"/></svg>`;

export const icons = {
  close: svg(`<path d="m4 4 8 8M12 4l-8 8"/>`),
  external: svg(`<path d="M9.5 2.5h4v4M13.5 2.5 8 8"/><path d="M12 9.5v2.25c0 1-.75 1.75-1.75 1.75h-6c-1 0-1.75-.75-1.75-1.75v-6c0-1 .75-1.75 1.75-1.75H6.5"/>`),
  collapse: svg(`<path d="m5 2.5 3 3 3-3M5 13.5l3-3 3 3"/>`),
  expand: svg(`<path d="m5 5.5 3-3 3 3M5 10.5l3 3 3-3"/>`),
  theme: svg(`<circle cx="8" cy="8" r="6"/><path d="M8 2a6 6 0 0 0 0 12z" fill="currentColor" stroke="none"/>`),
  settings: svg(`<path d="M2.5 4.5h6M11.5 4.5h2M2.5 11.5h2M7.5 11.5h6"/><circle cx="10" cy="4.5" r="1.5"/><circle cx="6" cy="11.5" r="1.5"/>`),
  files: svg(`<path d="M3 2v8.5c0 .8.7 1.5 1.5 1.5H7M3 5h4"/><rect x="7.5" y="3.25" width="6" height="3.5" rx="1"/><rect x="7.5" y="10.25" width="6" height="3.5" rx="1"/>`),
  info: svg(`<circle cx="8" cy="8" r="6"/><path d="M8 7.25V11M8 5v.01"/>`),
  search: svg(`<circle cx="7" cy="7" r="4.5"/><path d="m10.5 10.5 3 3"/>`),
  filter: svg(`<path d="M2 4.5h12M4 8h8M6 11.5h4"/>`),
  stats: svg(`<path d="M3 13.5V9M8 13.5V3M13 13.5V6.5"/>`),
  engine: svg(`<rect x="4" y="4" width="8" height="8" rx="1.5"/><path d="M6.5 1.5v2.5M9.5 1.5v2.5M6.5 12v2.5M9.5 12v2.5M1.5 6.5H4M1.5 9.5H4M12 6.5h2.5M12 9.5h2.5"/>`),
  arrow: svg(`<path d="M3 8h10M9 4l4 4-4 4"/>`),
  key: svg(`<circle cx="5" cy="11" r="2.5"/><path d="m7 9 6.5-6.5M11 4.5l1.75 1.75"/>`),
  check: svg(`<path d="m3.5 8.5 3 3 6-7"/>`),
  split: sprite("diff-split"),
  unified: sprite("diff-unified"),
  github: sprite("brand-github"),
  loading: `<svg viewBox="0 0 20 20" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" aria-hidden="true" class="spin"><path d="M10 2.5a7.5 7.5 0 1 0 7.5 7.5" /></svg>`,
};

/** The diffs sprite, once per page, for the icons above that `<use>` it. */
export function mountSprite() {
  if (document.querySelector("[data-diffs-sprite]")) return;
  const holder = document.createElement("div");
  holder.hidden = true;
  holder.dataset.diffsSprite = "";
  holder.innerHTML = SVGSpriteSheet;
  document.body.prepend(holder);
}
