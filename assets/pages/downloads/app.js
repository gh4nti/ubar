const list = document.getElementById("list");

function post(message) {
  window.webkit.messageHandlers.ubar.postMessage(message);
}

function formatBytes(bytes) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** index).toFixed(index ? 1 : 0)} ${units[index]}`;
}

window.ubarRenderDownloads = function renderDownloads(data) {
  const items = data.items || [];
  list.replaceChildren();

  if (!items.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = "No downloads yet.";
    list.append(empty);
    return;
  }

  items.forEach((item) => {
    const received = Number(item.received) || 0;
    const total = Number(item.total) || 0;
    const hasTotal = total > 0 && total >= received;
    const card = document.createElement("article");
    card.className = "row-card download-card";

    const head = document.createElement("div");
    head.className = "download-head";
    const name = document.createElement("strong");
    name.textContent = item.filename || item.uri;
    const status = document.createElement("span");
    status.className = `status-${item.status}`;
    status.textContent =
      item.status === "active"
        ? received > 0
          ? `${formatBytes(received)}${hasTotal ? ` of ${formatBytes(total)}` : " downloaded"}`
          : "Starting…"
        : item.status;
    head.append(name, status);

    const meta = document.createElement("div");
    meta.className = "download-meta";
    meta.textContent = item.uri;

    card.append(head, meta);

    if (item.status === "active") {
      const bar = document.createElement("div");
      bar.className = "progress";
      const fill = document.createElement("div");
      if (hasTotal && received > 0) {
        fill.style.width = `${Math.min(100, (received / total) * 100)}%`;
      } else {
        bar.classList.add("indeterminate");
      }
      bar.append(fill);
      card.append(bar);
    }

    const actions = document.createElement("div");
    actions.className = "download-actions";
    if (item.status === "active") {
      const cancel = document.createElement("button");
      cancel.className = "ghost";
      cancel.textContent = "Cancel";
      cancel.addEventListener("click", () => post(`download-cancel:${item.id}`));
      actions.append(cancel);
    }
    if (item.status === "done") {
      const open = document.createElement("button");
      open.textContent = "Open";
      open.addEventListener("click", () => post(`download-open:${item.id}`));
      actions.append(open);
    }
    card.append(actions);
    list.append(card);
  });
};

document.getElementById("open-folder").addEventListener("click", () => post("downloads-folder"));
document.getElementById("clear-all").addEventListener("click", () => post("downloads-clear"));
