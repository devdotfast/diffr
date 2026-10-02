export interface EnsureBinaryOptions {
  into?: string;
  full?: boolean;
  check?: boolean;
  required?: boolean;
  pins?: string;
}
export function ensureBinary(options?: EnsureBinaryOptions): Promise<string | undefined>;
