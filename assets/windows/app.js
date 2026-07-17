const send = (message) => window.ipc.postMessage(JSON.stringify(message));
const formatBytes = bytes => {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const unit = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${(bytes / 1024 ** unit).toFixed(unit ? 1 : 0)} ${units[unit]}`;
};
const tabs = document.getElementById('tab-strip');
const add = document.getElementById('new');
const dragRegion = document.getElementById('drag-region');
const address = document.getElementById('address');
const bookmark = document.getElementById('bookmark');
const downloadButton = document.getElementById('downloads');
const downloadPanel = document.getElementById('downloads-panel');
const downloadList = document.getElementById('downloads-list');
const downloadAll = document.getElementById('downloads-all');
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
const hideDownloads = () => {
  if (downloadPanel.hidden) return;
  downloadPanel.hidden = true;
  downloadButton.classList.remove('active');
  downloadButton.setAttribute('aria-expanded', 'false');
  send({cmd: 'menu-close'});
};
window.ubarShowDownloads = () => {
  hideMenu();
  downloadPanel.hidden = false;
  downloadButton.classList.add('active');
  downloadButton.setAttribute('aria-expanded', 'true');
  send({cmd: 'menu-open'});
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
  hideDownloads();
  toggleMenu();
});
downloadButton.addEventListener('click', event => {
  event.stopPropagation();
  if (downloadPanel.hidden) {
    window.ubarShowDownloads();
  } else {
    hideDownloads();
  }
});
downloadPanel.addEventListener('click', event => event.stopPropagation());
downloadAll.addEventListener('click', () => {
  hideDownloads();
  send({cmd: 'open-page', value: 'downloads'});
});
menu.addEventListener('click', event => event.stopPropagation());
menu.querySelectorAll('[data-page]').forEach(button => {
  button.addEventListener('click', () => choose({cmd: 'open-page', value: button.dataset.page}));
});
menu.querySelectorAll('[data-command]').forEach(button => {
  button.addEventListener('click', () => choose({cmd: button.dataset.command}));
});
document.addEventListener('click', () => { hideMenu(); hideDownloads(); });
address.addEventListener('keydown', event => {
  if (event.key === 'Enter') send({cmd: 'navigate', value: address.value});
});
dragRegion.addEventListener('pointerdown', event => {
  if (event.button === 0) send({cmd: 'window-drag'});
});
dragRegion.addEventListener('dblclick', () => send({cmd: 'window-maximize'}));
const resizeDirection = event => {
  const edge = 6;
  const left = event.clientX < edge;
  const right = event.clientX >= innerWidth - edge;
  const top = event.clientY < edge;
  return top ? (left ? 'nw' : right ? 'ne' : 'n') : left ? 'w' : right ? 'e' : '';
};
const resizeCursor = {n:'ns-resize',e:'ew-resize',w:'ew-resize',nw:'nwse-resize',ne:'nesw-resize'};
window.addEventListener('pointermove', event => {
  const direction = resizeDirection(event);
  document.documentElement.toggleAttribute('data-ubar-resize', !!direction);
  document.documentElement.style.setProperty('--ubar-resize-cursor', resizeCursor[direction] || '');
});
window.addEventListener('pointerdown', event => {
  if (event.button !== 0) return;
  const direction = resizeDirection(event);
  if (!direction) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  send({cmd: 'window-resize', value: direction});
}, true);
window.addEventListener('keydown', event => {
  if (event.key === 'Escape') {
    hideMenu();
    return;
  }
  if (!event.ctrlKey) return;
  const key = event.key.toLowerCase();
  if (key === 'tab' || /^[1-9]$/.test(key)) {
    event.preventDefault();
    send({cmd: 'shortcut', key, shift: event.shiftKey});
    return;
  }
  const commands = {
    l: () => window.ubarFocusAddress(),
    t: () => send({cmd: 'new-tab'}),
    w: () => send({cmd: 'shortcut', key: 'w'}),
    n: () => send({cmd: 'shortcut', key: 'n', shift: event.shiftKey}),
    j: () => send({cmd: 'shortcut', key: 'j'}),
    h: () => send({cmd: 'shortcut', key: 'h'}),
    ',': () => send({cmd: 'shortcut', key: ','}),
    o: () => send({cmd: 'shortcut', key: 'o', shift: true}),
    x: () => send({cmd: 'shortcut', key: 'x', shift: true}),
    '+': () => send({cmd: 'zoom-in'}),
    '=': () => send({cmd: 'zoom-in'}),
    '-': () => send({cmd: 'zoom-out'}),
    '0': () => send({cmd: 'zoom-reset'}),
  };
  const command = commands[event.key.toLowerCase()];
  const internalKey = ['h', ',', 'o', 'x', 'n'].includes(key);
  if (command && (!internalKey || event.shiftKey === ['o', 'x', 'n'].includes(key))) {
    event.preventDefault();
    command();
  }
});

window.ubarFocusAddress = () => {
  address.focus();
  address.select();
};
window.ubarFocusAddress();

window.ubarRender = (items, uri, bookmarked, zoom = 1, extensions = []) => {
  tabs.querySelectorAll('.tab').forEach(tab => tab.remove());
  for (const item of items) {
    const tab = document.createElement('button');
    tab.className = `tab${item.active ? ' active' : ''}${item.incognito ? ' incognito' : ''}`;
    tab.innerHTML = '<span class="title"></span><span class="close">&#xd7;</span>';
    const title = `${item.incognito ? 'Private - ' : ''}${item.title || 'New Tab'}`;
    tab.querySelector('.title').textContent = title;
    tab.title = title;
    tab.draggable = true;
    let suppressClickUntil = 0;
    tab.addEventListener('dragstart', event => {
      event.dataTransfer.effectAllowed = 'move';
      event.dataTransfer.setData('text/plain', String(item.id));
      tab.classList.add('dragging');
    });
    tab.addEventListener('dragend', () => {
      tab.classList.remove('dragging');
      suppressClickUntil = performance.now() + 250;
    });
    tab.addEventListener('dragover', event => {
      event.preventDefault();
      event.dataTransfer.dropEffect = 'move';
    });
    tab.addEventListener('drop', event => {
      event.preventDefault();
      const id = Number(event.dataTransfer.getData('text/plain'));
      if (Number.isSafeInteger(id) && id !== item.id) {
        send({cmd: 'move-tab', id, target: item.id,
          after: event.clientX >= tab.getBoundingClientRect().left + tab.offsetWidth / 2});
      }
    });
    tab.addEventListener('click', event => {
      if (performance.now() < suppressClickUntil) {
        event.preventDefault();
        return;
      }
      send({cmd: 'select-tab', id: item.id});
    });
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

window.ubarRenderDownloads = items => {
  downloadButton.hidden = items.length === 0;
  if (!items.length) hideDownloads();
  downloadList.replaceChildren();
  for (const item of items) {
    const row = document.createElement('div');
    row.className = 'download-item';
    const name = document.createElement('div');
    name.className = 'download-name';
    name.textContent = item.filename || 'download';
    name.title = name.textContent;
    const status = document.createElement('div');
    status.className = 'download-state';
    const received = Number(item.received) || 0;
    const total = Number(item.total) || 0;
    const hasTotal = total > 0 && total >= received;
    if (item.status === 'active') {
      const progress = document.createElement('progress');
      if (hasTotal && received > 0) {
        progress.max = total;
        progress.value = received;
      }
      status.textContent = received > 0
        ? `${formatBytes(received)}${hasTotal ? ` of ${formatBytes(total)}` : ' downloaded'}`
        : 'Starting…';
      status.prepend(progress);
    } else {
      status.textContent = item.status === 'done' ? 'Complete' : item.status;
    }
    const actions = document.createElement('div');
    actions.className = 'download-actions';
    const action = (label, cmd) => {
      const button = document.createElement('button');
      button.textContent = label;
      button.addEventListener('click', () => send({cmd, id: item.id}));
      actions.append(button);
    };
    if (item.status === 'active') action('Cancel', 'download-cancel');
    if (item.status === 'done') {
      action('Open', 'download-open');
      action('Show', 'download-show');
    }
    row.append(name, status, actions);
    downloadList.append(row);
  }
};

send({cmd: 'ready'});
