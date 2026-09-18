export function legacyRetry(request) {
  const attempts = 5;
  const history = [];
  let lastError;
  for (let attempt = 0; attempt < attempts; attempt++) {
    try {
      const response = request();
      history.push(response.status);
      if (response.ok) {
        return response;
      }
      lastError = new Error("search_token: removed failure path");
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError;
}
