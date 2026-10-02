/** The browser's stand-in for the TUI's terminal text module: rows.ts only measures cell widths. */
import stringWidth from "string-width";

export const measureTextWidth = (text: string) => stringWidth(text);
