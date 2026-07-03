const extensionsList = document.getElementById("extensions-list");
const browseStore = document.getElementById("browse-store");

function post(message) {
  window.webkit.messageHandlers.ubar.postMessage(message);
}

function entryRow(item) {
  const article = document.createElement("article");
  article.className = "row-card";

  const title = document.createElement("div");
  title.className = "row-link";
  title.textContent = item.name;
  if (item.page) {
    title.style.cursor = "pointer";
    title.title = "Open extension";
    title.addEventListener("click", () => post(`open:${encodeURIComponent(item.page)}`));
  }

  const meta = document.createElement("div");
  meta.className = "row-meta";
  meta.textContent = item.dir;

  const remove = document.createElement("button");
  remove.className = "ghost";
  remove.textContent = "Remove";
  remove.addEventListener("click", () => post(`delete-extension:${encodeURIComponent(item.name)}`));

  article.append(title, meta, remove);
  return article;
}

window.ubarRenderExtensions = function renderExtensions(data) {
  extensionsList.replaceChildren();

  if (!data.items.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent =
      "No extensions installed. Visit addons.mozilla.org or chromewebstore.google.com and use the install button.";
    extensionsList.append(empty);
    return;
  }

  data.items.forEach((item) => extensionsList.append(entryRow(item)));
};

browseStore.addEventListener("click", () =>
  post(`open:${encodeURIComponent("https://addons.mozilla.org/en-US/firefox/extensions/")}`)
);
