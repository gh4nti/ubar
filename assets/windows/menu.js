const send = (message) => window.ipc.postMessage(JSON.stringify(message));
const choose = (message) => {
  send(message);
  send({cmd: 'hide-menu'});
};

document.querySelectorAll('[data-page]').forEach(button => {
  button.addEventListener('click', () => choose({cmd: 'open-page', value: button.dataset.page}));
});
document.querySelectorAll('[data-command]').forEach(button => {
  button.addEventListener('click', () => choose({cmd: button.dataset.command}));
});
window.addEventListener('keydown', event => {
  if (event.key === 'Escape') send({cmd: 'hide-menu'});
});
