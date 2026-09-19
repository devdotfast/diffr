export function retry(request, options) {
  const attempts = options.attempts ?? 3; // search_token: changed configuration
  const history = []; // search_token: unchanged line inside changed function
  let lastError;
  for (let attempt = 0; attempt < attempts; attempt++) {
    try {
      const response = request();
      history.push(response.status);
      if (response.ok) {
        return response;
      }
      lastError = new Error(response.statusText);
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError;
}

export function describeRetry() {
  const label = "search_token: unchanged explanation";
  return label;
}
