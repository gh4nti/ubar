const send = (message) => window.ipc.postMessage(JSON.stringify(message));
const tabs = document.getElementById('tab-strip');
const add = document.getElementById('new');
const dragRegion = document.getElementById('drag-region');
const address = document.getElementById('address');
const bookmark = document.getElementById('bookmark');
const extensionActions = document.getElementById('extension-actions');
const more = document.getElementById('more');
const menu = document.getElementById('app-menu');
const zoomValue = document.getElementById('zoom-value');
const hideMenu = () => {
  if (menu.hidden) return;
  menu.hidden = true;
  more.classList.remove('active');
  send({cmd: 'menu-close'});
};
const toggleMenu = () => {
  menu.hidden = !menu.hidden;
  more.classList.toggle('active', !menu.hidden);
  send({cmd: menu.hidden ? 'menu-close' : 'menu-open'});
};
const choose = (message) => {
  send(message);
  hideMenu();
};

document.querySelectorAll('#nav [data-command], #window-controls [data-command]').forEach(button => {
  button.addEventListener('click', () => send({cmd: button.dataset.command}));
});
add.addEventListener('click', () => send({cmd: 'new-tab'}));
bookmark.addEventListener('click', () => send({cmd: 'bookmark'}));
more.addEventListener('click', event => {
  event.stopPropagation();
  toggleMenu();
});
menu.addEventListener('click', event => event.stopPropagation());
menu.querySelectorAll('[data-page]').forEach(button => {
  button.addEventListener('click', () => choose({cmd: 'open-page', value: button.dataset.page}));
});
menu.querySelectorAll('[data-command]').forEach(button => {
  button.addEventListener('click', () => choose({cmd: button.dataset.command}));
});
document.addEventListener('click', hideMenu);
address.addEventListener('keydown', event => {
  if (event.key === 'Enter') send({cmd: 'navigate', value: address.value});
});
dragRegion.addEventListener('pointerdown', event => {
  if (event.button === 0) send({cmd: 'window-drag'});
});
dragRegion.addEventListener('dblclick', () => send({cmd: 'window-maximize'}));
window.addEventListener('keydown', event => {
  if (event.key === 'Escape') {
    hideMenu();
    return;
  }
  if (!event.ctrlKey) return;
  const commands = {
    l: () => window.ubarFocusAddress(),
    t: () => send({cmd: 'new-tab'}),
    '+': () => send({cmd: 'zoom-in'}),
    '=': () => send({cmd: 'zoom-in'}),
    '-': () => send({cmd: 'zoom-out'}),
    '0': () => send({cmd: 'zoom-reset'}),
  };
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

window.ubarRender = (items, uri, bookmarked, zoom = 1, extensions = []) => {
  tabs.querySelectorAll('.tab').forEach(tab => tab.remove());
  for (const item of items) {
    const tab = document.createElement('button');
    tab.className = `tab${item.active ? ' active' : ''}${item.incognito ? ' incognito' : ''}`;
    tab.innerHTML = '<span class="title"></span><span class="close">&#xd7;</span>';
    const title = `${item.incognito ? 'Private - ' : ''}${item.title || 'New Tab'}`;
    tab.querySelector('.title').textContent = title;
    tab.title = title;
    tab.addEventListener('click', () => send({cmd: 'select-tab', id: item.id}));
    tab.addEventListener('auxclick', event => {
      if (event.button === 1) {
        event.preventDefault();
        send({cmd: 'close-tab', id: item.id});
      }
    });
    tab.querySelector('.close').addEventListener('click', event => {
      event.stopPropagation();
      send({cmd: 'close-tab', id: item.id});
    });
    tabs.insertBefore(tab, add);
  }
  if (document.activeElement !== address) address.value = uri.includes('ubar.localhost') || uri.startsWith('ubar:') ? '' : uri;
  bookmark.classList.toggle('active', bookmarked);
  bookmark.innerHTML = bookmarked ? '&#x2605;' : '&#x2606;';
  zoomValue.textContent = `${Math.round(zoom * 100)}%`;
  extensionActions.replaceChildren();
  for (const extension of extensions) {
    const button = document.createElement('button');
    button.className = 'extension-action';
    button.textContent = (extension.name || '?').slice(0, 1).toUpperCase();
    button.title = extension.name;
    button.addEventListener('click', () => send({cmd: 'open-extension', value: extension.page}));
    extensionActions.append(button);
  }
};

send({cmd: 'ready'});
