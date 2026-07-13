const send = (message) => window.ipc.postMessage(JSON.stringify(message));
const tabs = document.getElementById('tabs');
const add = document.getElementById('new');
const address = document.getElementById('address');
const bookmark = document.getElementById('bookmark');

document.querySelectorAll('[data-command]').forEach(button => {
  button.addEventListener('click', () => send({cmd: button.dataset.command}));
});
add.addEventListener('click', () => send({cmd: 'new-tab'}));
bookmark.addEventListener('click', () => send({cmd: 'bookmark'}));
address.addEventListener('keydown', event => {
  if (event.key === 'Enter') send({cmd: 'navigate', value: address.value});
});
window.addEventListener('keydown', event => {
  if (!event.ctrlKey) return;
  const commands = {l: () => window.ubarFocusAddress(), t: () => send({cmd: 'new-tab'})};
  const command = commands[event.key.toLowerCase()];
  if (command) {
    event.preventDefault();
    command();
  }
});

window.ubarFocusAddress = () => {
  address.focus();
  address.select();
};

window.ubarRender = (items, uri, bookmarked) => {
  tabs.querySelectorAll('.tab').forEach(tab => tab.remove());
  for (const item of items) {
    const tab = document.createElement('button');
    tab.className = `tab${item.active ? ' active' : ''}`;
    tab.innerHTML = '<span class="title"></span><span class="close">&#xd7;</span>';
    tab.querySelector('.title').textContent = item.title || 'New Tab';
    tab.title = item.title || 'New Tab';
    tab.addEventListener('click', () => send({cmd: 'select-tab', id: item.id}));
    tab.querySelector('.close').addEventListener('click', event => {
      event.stopPropagation();
      send({cmd: 'close-tab', id: item.id});
    });
    tabs.insertBefore(tab, add);
  }
  if (document.activeElement !== address) address.value = uri.includes('ubar.localhost') || uri.startsWith('ubar:') ? '' : uri;
  bookmark.classList.toggle('active', bookmarked);
  bookmark.innerHTML = bookmarked ? '&#x2605;' : '&#x2606;';
};

send({cmd: 'ready'});
