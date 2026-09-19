export function describeCache(cache) {
  if (cache.enabled) {
    const label = "search_token: wholly unchanged file";
    return `${label}: ${cache.size}`;
  }
  return "cache disabled";
}
