const homepageInput = document.getElementById("homepage-input");
const saveHomepage = document.getElementById("save-homepage");
const useNewTab = document.getElementById("use-new-tab");
const historyCount = document.getElementById("history-count");
const bookmarkCount = document.getElementById("bookmark-count");

function post(message) {
  window.webkit.messageHandlers.ubar.postMessage(message);
}

window.ubarRenderSettings = function renderSettings(data) {
  homepageInput.value = data.homepageUri || "";
  historyCount.textContent = String(data.historyCount);
  bookmarkCount.textContent = String(data.bookmarkCount);
};

saveHomepage.addEventListener("click", () => {
  post(`save-homepage:${encodeURIComponent(homepageInput.value.trim())}`);
});

useNewTab.addEventListener("click", () => {
  homepageInput.value = "";
  post("save-homepage:");
});
