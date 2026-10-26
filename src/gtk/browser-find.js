// Runs in Worminal's isolated WebKit world; native find still owns matching and scrolling.
function update(action) {
  if (action !== "clear") update("clear");
  function visit(document) {
    document.querySelectorAll("[data-worminal-find]").forEach(node => node.remove());
    const view = document.defaultView;
    view?.CSS?.highlights?.delete("worminal-find");
    const selection = document.getSelection();
    if (action !== "clear" && selection?.rangeCount && !selection.isCollapsed) {
      const range = selection.getRangeAt(0).cloneRange();
      const style = document.createElement("style");
      style.dataset.worminalFind = "";
      style.textContent = "::selection { background: #ffdd00 !important; color: #000 !important; }" +
        "::highlight(worminal-find) { background: #ffdd00; color: #000; text-decoration: underline 3px #000; }" +
        "a:focus-visible { outline: 3px solid #ffdd00 !important; outline-offset: 2px !important; }";
      document.documentElement.append(style);
      if (action === "accept") {
        const node = range.startContainer;
        const element = node.nodeType === Node.ELEMENT_NODE ? node : node.parentElement;
        const link = element?.closest("a[href]");
        // Focusing the containing link lets the browser's normal Enter action open it.
        link?.focus({ preventScroll: true });
        selection.removeAllRanges();
        selection.addRange(range);
      }
      if (view?.CSS?.highlights) view.CSS.highlights.set("worminal-find", new view.Highlight(range));
      return true;
    }
    for (const frame of document.querySelectorAll("iframe, frame")) {
      try {
        const child = frame.contentDocument;
        if (child && visit(child)) return true;
      } catch { /* Cross-origin frames retain WebKit's native find selection. */ }
    }
    return false;
  }
  visit(document);
}

globalThis.WorminalFind = { update };
