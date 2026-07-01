const bookmarkList = document.getElementById("bookmark-list");

function post(message) {
  window.webkit.messageHandlers.ubar.postMessage(message);
}

function bookmarkRow(item) {
  const article = document.createElement("article");
  article.className = "row-card row-actions";

  const left = document.createElement("div");

  const title = document.createElement("button");
  title.className = "row-link";
  title.textContent = item.title || item.uri;
  title.addEventListener("click", () => post(`open:${encodeURIComponent(item.uri)}`));

  const meta = document.createElement("div");
  meta.className = "row-meta";
  meta.textContent = item.uri;

  left.append(title, meta);

  const remove = document.createElement("button");
  remove.className = "ghost";
  remove.textContent = "Remove";
  remove.addEventListener("click", () => post(`delete-bookmark:${encodeURIComponent(item.uri)}`));

  article.append(left, remove);
  return article;
}

window.ubarRenderBookmarks = function renderBookmarks(data) {
  bookmarkList.replaceChildren();

  if (!data.items.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = "No bookmarks yet.";
    bookmarkList.append(empty);
    return;
  }

  data.items.forEach((item) => bookmarkList.append(bookmarkRow(item)));
};
