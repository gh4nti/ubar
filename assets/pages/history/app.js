const historyList = document.getElementById("history-list");
const clearHistory = document.getElementById("clear-history");

function post(message) {
  window.webkit.messageHandlers.ubar.postMessage(message);
}

function entryRow(item) {
  const article = document.createElement("article");
  article.className = "row-card";

  const title = document.createElement("button");
  title.className = "row-link";
  title.textContent = item.title || item.uri;
  title.addEventListener("click", () => post(`open:${encodeURIComponent(item.uri)}`));

  const meta = document.createElement("div");
  meta.className = "row-meta";
  meta.textContent = `${item.uri}  •  ${item.time}`;

  article.append(title, meta);
  return article;
}

window.ubarRenderHistory = function renderHistory(data) {
  historyList.replaceChildren();

  if (!data.items.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = "No history yet.";
    historyList.append(empty);
    return;
  }

  data.items.forEach((item) => historyList.append(entryRow(item)));
};

clearHistory.addEventListener("click", () => post("clear-history"));
