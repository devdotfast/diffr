export type Edition = "lean" | "full";

/** Downloads only when requested. Returns an absolute executable path. */
export function ensureBinary(options: {
  directory: string;
  edition?: Edition;
  /** Verify a cached installation without downloading. */
  check?: boolean;
  /** Override the package's pinned release catalog. */
  pins?: string;
}): Promise<string>;
