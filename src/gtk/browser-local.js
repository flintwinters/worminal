// Only the explicitly selected local document gets this closure. Subframes cannot read its credential.
(() => {
  const document = "%s", credential = "%s";
  const current = new URL(location.href);
  current.search = "";
  current.hash = "";
  if (current.href !== document) return;
  Object.defineProperty(window, "worminal", { value: Object.freeze({
    async readDirectory(path) {
      const query = new URLSearchParams({ path, credential });
      const response = await fetch("worminal:///list?" + query);
      if (!response.ok) throw new Error("Could not read folder.");
      return response.json();
    }
  }) });
})();
