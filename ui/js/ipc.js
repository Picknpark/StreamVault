// Safe Tauri 2 IPC & Event wrapper (Plain ES Module — no bundler or framework required)

export async function invokeCmd(command, args = {}) {
  if (window.__TAURI__ && window.__TAURI__.core && typeof window.__TAURI__.core.invoke === "function") {
    return window.__TAURI__.core.invoke(command, args);
  }
  if (window.__TAURI_INTERNALS__ && typeof window.__TAURI_INTERNALS__.invoke === "function") {
    return window.__TAURI_INTERNALS__.invoke(command, args);
  }
  throw new Error(`Tauri IPC unavailable for command: ${command}`);
}

export async function listenEvent(eventName, handler) {
  if (window.__TAURI__ && window.__TAURI__.event && typeof window.__TAURI__.event.listen === "function") {
    return window.__TAURI__.event.listen(eventName, (ev) => handler(ev.payload));
  }
  return () => {};
}
