const homepageInput = document.getElementById("homepage-input");
const saveHomepage = document.getElementById("save-homepage");
const useNewTab = document.getElementById("use-new-tab");
const historyCount = document.getElementById("history-count");
const bookmarkCount = document.getElementById("bookmark-count");
const siteCount = document.getElementById("site-count");
const defaultPermissions = document.getElementById("default-permissions");
const siteOrigin = document.getElementById("site-origin");
const sitePermission = document.getElementById("site-permission");
const siteValue = document.getElementById("site-value");
const saveSitePermission = document.getElementById("save-site-permission");
const siteList = document.getElementById("site-list");

const permissions = [
  ["notifications", "Notifications", "Let sites show notification prompts."],
  ["location", "Location", "Let sites request your approximate location."],
  ["cookies", "Cookies", "Allow or block site cookie storage."],
  ["camera", "Camera", "Let sites request camera access."],
  ["microphone", "Microphone", "Let sites request microphone access."],
  ["javascript", "JavaScript", "Allow pages to run scripts."],
  ["popups", "Pop-ups and redirects", "Allow or block disruptive windows and redirects."],
];

function post(message) {
  window.webkit.messageHandlers.ubar.postMessage(message);
}

function behaviorSelect(value, onChange) {
  const select = document.createElement("select");
  ["ask", "allow", "block"].forEach((optionValue) => {
    const option = document.createElement("option");
    option.value = optionValue;
    option.textContent = optionValue[0].toUpperCase() + optionValue.slice(1);
    option.selected = optionValue === value;
    select.append(option);
  });
  select.addEventListener("change", () => onChange(select.value));
  return select;
}

function renderDefaultPermissions(defaults = {}) {
  defaultPermissions.replaceChildren();

  permissions.forEach(([key, label, description]) => {
    const row = document.createElement("div");
    row.className = "permission-row";

    const copy = document.createElement("div");
    const title = document.createElement("strong");
    const desc = document.createElement("span");
    title.textContent = label;
    desc.textContent = description;
    copy.append(title, desc);

    row.append(
      copy,
      behaviorSelect(defaults[key] || "ask", (value) => post(`save-permission-default:${key}:${value}`)),
    );
    defaultPermissions.append(row);
  });
}

function renderSiteList(sites = []) {
  siteList.replaceChildren();
  siteCount.textContent = String(sites.length);

  if (!sites.length) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = "No custom site rules yet.";
    siteList.append(empty);
    return;
  }

  sites.forEach((site) => {
    const card = document.createElement("article");
    card.className = "site-card";

    const top = document.createElement("div");
    top.className = "site-card-head";
    const origin = document.createElement("strong");
    origin.textContent = site.origin;
    const remove = document.createElement("button");
    remove.className = "ghost";
    remove.textContent = "Reset";
    remove.addEventListener("click", () => post(`remove-site:${encodeURIComponent(site.origin)}`));
    top.append(origin, remove);

    const grid = document.createElement("div");
    grid.className = "site-rule-grid";
    permissions.forEach(([key, label]) => {
      const rule = document.createElement("div");
      rule.className = "site-rule";
      const name = document.createElement("span");
      name.textContent = label;
      rule.append(
        name,
        behaviorSelect(site[key] || "ask", (value) => {
          post(`save-site-permission:${encodeURIComponent(site.origin)}:${key}:${value}`);
        }),
      );
      grid.append(rule);
    });

    card.append(top, grid);
    siteList.append(card);
  });
}

window.ubarRenderSettings = function renderSettings(data) {
  homepageInput.value = data.homepageUri || "";
  historyCount.textContent = String(data.historyCount);
  bookmarkCount.textContent = String(data.bookmarkCount);
  renderDefaultPermissions(data.defaults || {});
  renderSiteList(data.sites || []);
};

permissions.forEach(([key, label]) => {
  const option = document.createElement("option");
  option.value = key;
  option.textContent = label;
  sitePermission.append(option);
});

saveHomepage.addEventListener("click", () => {
  post(`save-homepage:${encodeURIComponent(homepageInput.value.trim())}`);
});

useNewTab.addEventListener("click", () => {
  homepageInput.value = "";
  post("save-homepage:");
});

saveSitePermission.addEventListener("click", () => {
  const origin = siteOrigin.value.trim().replace(/\/$/, "");
  if (!origin) {
    siteOrigin.focus();
    return;
  }
  post(`save-site-permission:${encodeURIComponent(origin)}:${sitePermission.value}:${siteValue.value}`);
});
